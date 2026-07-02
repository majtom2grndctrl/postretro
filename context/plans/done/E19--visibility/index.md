# postretro-visibility

> Epic: `E19--render-stack-decomposition`. Folds `compile-time-reduction` Task 3.

## Goal

Extract runtime portal traversal and frustum visibility into a CPU-only crate so editing visibility code does not recompile the renderer's GPU modules.

## Scope

### In scope
- `postretro-visibility`: move `visibility.rs` + `portal_vis.rs` — `VisibleCells`, `CameraCullVisibility`, `VisibilityStats`, `VisibilityPath`, `VisibilityResult`, `determine_visible_cells`, `extract_frustum_planes`, `Frustum`, `FrustumPlane`, `portal_traverse`, `narrow_frustum`, `clip_polygon_to_frustum`, and the internal clipping/frustum helpers (`is_aabb_outside_frustum`, `slide_near_plane_to`, `NEAR_PLANE_INDEX`) + their tests. `Frustum`/`FrustumPlane` are already `pub` in `visibility.rs` (widened in place by `E19--leaf-hygiene-and-boundary-prep`, not homed in render-data), so the cross-crate move compiles; the `pub(crate)` helpers travel with the code and stay crate-visible (both files co-move — no widening needed). `extract_frustum_planes` is `pub` and called from `main.rs:2159`, so re-export it (and the other public symbols) at the `postretro_visibility::` crate root.
- Deps (authoritative list in Task 1): normal — `postretro-level-loader` (`LevelWorld`, `CellData`, `PortalData`) with default features disabled + `glam` + `log` (preserves existing moved-file diagnostics); dev-only — `postretro-render-data` **and** `postretro-level-format`. The normal build carries neither dev edge.
- Re-point all in-binary importers of the moved symbols to `postretro_visibility::` and delete the two `mod` decls (mechanics + file list in Task 1). Leave the unrelated `render/ui/tree/tests/visibility.rs` untouched.

### Out of scope
- The GPU cull pipelines (`compute_cull`/`candidate_cull`/`shadow_cull`) — those are GPU and move into `postretro-renderer` (`E19--renderer-gpu`).
- Frame-level render policy that reads `FullRenderer` state (stays renderer-side).

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md`. Durable decisions are captured into `context/lib/` per spec as each spec is approved — not in one batch at first promotion.
- [ ] Crate is a workspace member (added to `Cargo.toml` `[workspace] members`); `cargo build --workspace` + `cargo test --workspace` pass; visibility/portal tests pass from their relocated home, including lightweight portal fixtures (no full GPU `Renderer` needed).
- [ ] `cargo tree -p postretro-visibility` shows no wgpu/winit/glyphon/kira/mlua/rquickjs. The normal-build graph carries no `postretro-render-data` **or** `postretro-level-format` edge — both are dev-dependencies (`cargo tree -p postretro-visibility --edges normal` omits both; they appear only under `--edges dev`).
- [ ] (Review/CI gate, not a harness-runnable test.) Editing the visibility crate and running its tests recompiles no `wgpu`/`naga`/`winit`/`kira` (the epic's cpu-only firewall AC; the renderer GPU modules sit downstream of that toolchain). Structurally implied by the dev-only edges above.
- [ ] The relocated visibility/portal unit tests pass from `postretro-visibility` with fixture-only adaptations needed by the new crate boundary. Runtime visibility behavior stays unchanged; the in-file tests remain the fixture and no separate golden corpus is required.
- [ ] (Manual PR-artifact gate.) Before/after warm-edit timing captured for a `portal_vis.rs`/`visibility.rs` touch vs. the `E19--baseline-and-cargo-config` baseline (epic global AC; Milestone 1 names this touch specifically).

## Tasks

### Task 1: Extract postretro-visibility
Create the crate, move `crates/postretro/src/visibility.rs` + `portal_vis.rs` into it (both files co-move into one crate), wire deps, re-export the public surface, and re-point every importer. The `Frustum`/`FrustumPlane`/`extract_frustum_planes` symbols the move relies on are already `pub` in `visibility.rs` (widened in place by `E19--leaf-hygiene-and-boundary-prep`); the `pub(crate)` helpers (`is_aabb_outside_frustum`, `slide_near_plane_to`, `NEAR_PLANE_INDEX`) travel with the code and stay crate-visible — no widening needed.

**Deps.** `[dependencies]`: `glam`, `log`, `postretro-level-loader` with default features disabled (for the slim `LevelWorld`, `CellData`, and portal/cell data surface — depend on `LevelWorld` directly). `[dev-dependencies]`: `postretro-render-data` **and** `postretro-level-format`. Neither dev crate may be a normal dep (AC2 gates both under `--edges normal`). `postretro-level-loader` keeps its default full PRL-load feature for existing consumers; `postretro-visibility` opts out so its normal graph does not inherit render-data or level-format.

**Re-exports + importers.** Declare both modules in the crate's `lib.rs` and expose the externally-consumed symbols at the `postretro_visibility::` crate root: `VisibleCells`, `CameraCullVisibility`, `VisibilityPath`, `VisibilityResult`, `VisibilityStats`, `determine_visible_cells`, `extract_frustum_planes` (consumed at `main.rs:120-122,2141,2159`). In the binary, delete `mod visibility;`/`mod portal_vis;` (`main.rs:51`/`main.rs:38`) and re-point every `crate::visibility::`/`crate::portal_vis::` **and** bare `visibility::`/`portal_vis::` path (e.g. `main.rs:2141,2159`) to `postretro_visibility::`. That is 11 in-binary importer files — including the still-in-binary GPU cull modules `compute_cull.rs` and `candidate_cull_mirror.rs` (they import `VisibleCells`; they don't leave until `E19--renderer-gpu`) and `scripting/systems/mesh_render.rs`. Lean on `cargo build --workspace` to surface every stale path; a `crate::`-anchored grep misses the bare `main.rs` calls, so the compiler — not grep — is the completion gate. Leave the unrelated `render/ui/tree/tests/visibility.rs` untouched.

**Test fixups on move.** `portal_vis.rs` tests read `crate::camera::{HFOV, NEAR, FAR}` via `use crate::camera;` (`portal_vis.rs:1789,1893`) plus qualified uses (`:1828,1830,1935,1937`). `camera` stays in the binary and does not move — drop the `use` lines and inline test-local consts in the relocated file: `HFOV = 100°` in radians, `NEAR = 0.1`, `FAR = 4096.0` (source: `camera.rs:7-10`), or the relocated tests won't compile.

**Decision (was open question 6): depend on `LevelWorld` directly.** Principle: lean — the crate boundary already cuts the recompile coupling, so a borrowed portal-world view would be speculative abstraction.

**Deferred optimization:** the old draft's borrowed portal-world view (exposing only leaves/portals/adjacency) — add only on a measured rebuild problem.

## Sequencing
**Phase 1:** Task 1. Preconditions (all landed first): `E19--render-data` and `E19--level-loader` (level-loader depends on render-data), plus `E19--leaf-hygiene-and-boundary-prep` (the `Frustum`/`FrustumPlane` widening). Needs `postretro-level-loader` (`E19--level-loader`) as a normal dep; `postretro-render-data` (`E19--render-data`) and `postretro-level-format` are dev-dependencies only (test-side). Milestone 1.

**Downstream consumer.** `E19--render-cpu` (Milestone 2) will depend on `postretro-visibility` — `mesh_visible` (`render/mesh_pass.rs`) is a pure `LevelWorld`+`VisibleCells` predicate that descends into `postretro-render-cpu` (epic Decision 5). Keep `VisibleCells` (and the types the predicate reads) public.
