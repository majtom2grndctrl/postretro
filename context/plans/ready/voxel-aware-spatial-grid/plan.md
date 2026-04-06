# Voxel-Aware Spatial Grid

> **Status:** ready for implementation.
> **Depends on:** Voxel-Based PVS Rework (complete). VoxelGrid struct exists and is working.
> **Related:** `context/lib/build_pipeline.md`, `context/lib/development_guide.md`, `context/lib/testing_guide.md`
> **Note:** `context/lib/rendering_pipeline.md` is legacy while we experiment with new visibility solutions. Do not treat it as authoritative for PVS behavior.

---

## Goal

Make the spatial grid voxel-aware so that grid cells are classified as solid, air, or boundary using the VoxelGrid bitmap. This fixes two failing contract tests:

- `spatial_stability_within_room` — air-space points fall outside all cluster AABBs (coverage gaps)
- `z_shaped_rooms_a_and_c_not_mutually_visible` — clusters span across walls (false visibility)

---

## Background

Research into how established engines solve precomputed visibility (see `context/reference/other-engine-research.md`) identified a common pattern: use a volumetric solid/empty classification to inform spatial subdivision, rather than relying on a uniform grid or surface-only BSP.

**Key findings that inform this plan:**

- **Unreal's Precomputed Visibility Volumes** discard cells whose centers are inside geometry using collision overlap queries. We do the same with our VoxelGrid — `is_point_solid()` replaces Unreal's collision test.
- **Unity's Adaptive Probe Volumes** use a brick-based octree that places dense bricks near surfaces and sparse bricks in open air. Our approach is similar but simpler: classify uniform grid cells using voxel data, then subdivide only the boundary cells that straddle walls.
- **The ray tracing community's standard point-in-convex-hull test** (dot product against all half-planes) is already implemented in our `VoxelGrid::from_brushes()`. This is the same test used by Quake's `CM_PointContents`.
- **Adaptive spatial subdivision** (k-d tree, octree) at solid/empty transitions is a well-established technique. Our boundary cell subdivision is one level of this — binary split along the axis with the most solid-to-empty transitions.

The VoxelGrid already exists and provides the `is_solid(x, y, z)` and `is_point_solid(world_pos)` queries we need. This plan uses that existing infrastructure to make the spatial grid smarter — no new algorithms, just applying the voxel data we already have.

---

## Scope

### In scope

- Cell classification using VoxelGrid (solid / air / boundary)
- Solid cell skipping (no cluster created for fully-solid cells)
- Air cell retention (empty-air cells become clusters even with no faces)
- Boundary cell subdivision (cells straddling walls are split)
- Pipeline change: move VoxelGrid construction earlier, share between spatial grid and PVS
- Zero-face cluster handling in geometry extraction and packing

### Out of scope

- VoxelGrid changes (it already has the API we need)
- PVS algorithm changes
- .prl format changes
- Engine changes
- Consolidating duplicated `grid_cells_to_clusters` (separate cleanup task)

---

## Shared Context

All tasks operate within `postretro-level-compiler`. Key types:

- `VoxelGrid` (`src/visibility/voxel_grid.rs`) — 3D bitmap, `is_solid(x, y, z)` and `is_point_solid(world_pos)`. Currently private to the `visibility` module; Task 1 moves it to crate root.
- `GridCell` (`src/spatial_grid.rs`) — spatial grid cell with face indices and bounds.
- `SpatialGridResult` (`src/spatial_grid.rs`) — output of `assign_to_grid`.
- `Cluster` (`src/partition/types.rs`) — downstream: id, bounds, face_indices.
- `grid_cells_to_clusters` — duplicated in `main.rs`, `visibility.rs` tests, `geometry.rs` tests, `pack.rs` tests. All filter out empty cells. This filtering is the root cause of air-space coverage gaps. Duplication is a known issue but deferred to a cleanup task — for now, update each copy to handle air cells.

---

## Tasks

### Task 1: Move VoxelGrid + Pipeline Reorder

**Files:**
- `src/visibility/voxel_grid.rs` → `src/voxel_grid.rs`
- `src/main.rs` — add `pub mod voxel_grid;`, build VoxelGrid before spatial grid
- `src/visibility.rs` — change `mod voxel_grid;` to `use crate::voxel_grid;`, accept `&VoxelGrid` parameter in `compute_visibility` instead of building it internally
- `src/visibility/pvs.rs` — update import path

This is a pure refactor — behavior must be identical. The VoxelGrid moves from a private module inside `visibility` to the crate root, and its construction moves earlier in the pipeline so it can be shared.

**Pipeline after:**
```
parse → voxelize(brush_volumes) → assign_to_grid(faces) → grid_cells_to_clusters → compute_visibility(&voxel_grid) → geometry → pack
```

(Task 2 will add voxel_grid to `assign_to_grid`; this task just gets it to the right place.)

**Acceptance criteria:**
- `VoxelGrid` importable as `crate::voxel_grid::VoxelGrid` from any module in the crate.
- Compiler produces byte-identical .prl output for `test_map_4.map` before and after (same PVS, same geometry — this is a refactor, not a behavior change).
- All existing tests pass with same results (98 pass, 2 fail).
- `cargo check` and `cargo test` pass.

**Depends on:** nothing.

---

### Task 2: Cell Classification + Air Cell Retention

**File:** `postretro-level-compiler/src/spatial_grid.rs`, `src/main.rs`

Add voxel-based cell classification, skip solid cells, keep air cells as clusters.

**What to build:**

- `enum CellType { Solid, Air, Boundary }` — cell classification.
- `fn classify_cell(cell_bounds: &Aabb, voxel_grid: &VoxelGrid) -> CellType` — sample voxels overlapping the cell. All solid → `Solid`. All empty → `Air`. Mixed → `Boundary`. Use exact thresholds (100%/0%).
- Modify `assign_to_grid` to accept `Option<&VoxelGrid>`. When provided: classify cells, exclude `Solid` cells from output, include `Air` cells even with no faces. `Boundary` cells included as-is for now (subdivision in Task 3). When `None`: existing behavior unchanged.
- Update `main.rs` to pass `Some(&voxel_grid)` to `assign_to_grid`.
- Update each copy of `grid_cells_to_clusters` to keep air cells (cells with no faces but valid bounds). Currently they filter these out.
- Verify that `geometry::extract_geometry` and `pack::pack_and_write` handle zero-face clusters without panicking. A cluster with `face_start=N, face_count=0` must be valid in the PRL output.

**Tests (written in this task, alongside implementation):**

1. `solid_cells_excluded_from_clusters` — Box brush fills a region. Cells whose voxels are all solid should not become clusters.
2. `air_cells_retained_as_clusters_even_without_faces` — Hollow room (wall brushes enclosing air). Interior grid cells with no face centroids should still become clusters with valid bounds and empty `face_indices`.
3. `cell_classification_correct_for_mixed_geometry` — Room with walls. Classify all cells. Verify: exterior = solid, interior = air, wall-straddling = boundary. No single type should be >90% of all cells (sanity check that classification isn't degenerate).
4. `zero_face_cluster_survives_pack_round_trip` — A cluster with zero faces should serialize to .prl and deserialize without error. Verify `face_count=0` is valid downstream.

**Acceptance criteria:**
- All 4 new unit tests pass.
- `spatial_stability_within_room` contract test passes (air-space points now resolve to clusters).
- Corridor visibility contract tests still pass — air cells must not break PVS for connected rooms (`corridor_connected_rooms_are_mutually_visible`, `l_shaped_corridor_rooms_see_corridor`, `long_corridor_middle_sees_both_ends`, `small_brush_in_corridor_does_not_block_visibility`).
- Compiler produces valid .prl from `test_map_4.map`.
- Cluster count logged — compare to pre-change count. Expect more clusters (air cells now included) but not an explosion (solid cells excluded). Log should show cell classification breakdown (N solid skipped, N air retained, N boundary).
- `cargo check` and `cargo test` pass. At most 1 failure remaining (`z_shaped_rooms_a_and_c_not_mutually_visible`).

**Depends on:** Task 1.

---

### Task 3: Boundary Cell Subdivision

**File:** `postretro-level-compiler/src/spatial_grid.rs`

Subdivide boundary cells so no single cluster spans both sides of a wall.

**What to build:**

- `fn subdivide_boundary_cell(cell, voxel_grid, faces) -> Vec<GridCell>` — binary split along the axis with the sharpest solid-to-empty transition. For each resulting sub-cell, classify again. If still boundary, split once more (max depth 2). Discard solid sub-cells. Keep air sub-cells. Reassign faces to sub-cells by centroid. A face whose centroid falls in a solid sub-cell goes to the nearest air sub-cell (faces are on surfaces near solid/air boundaries — don't lose them).

- Integrate into `assign_to_grid`: after initial classification, replace each boundary cell with its subdivisions.

**Why binary split over octree:** A boundary cell typically straddles one wall. Splitting along the wall-normal axis produces 2 sub-cells that cleanly separate the two sides. An octree split produces 8 sub-cells — 6 of which are unnecessary splits along axes where there's no wall. Binary split is simpler, produces fewer clusters, and aligns better with the geometry.

**Tests:**

1. `boundary_cells_subdivided_into_solid_and_air` — Wall brush bisects a grid cell. After subdivision, no resulting cluster should span both sides. Verify by checking that no cluster's AABB contains air-space points on both sides of the wall.
2. `subdivision_does_not_explode_cluster_count` — Run subdivision on a complex geometry (the z-shaped three-room layout). Total cluster count should be less than 2x the pre-subdivision count. Subdivision should add precision at wall boundaries, not blow up everywhere.

**Acceptance criteria:**
- Both new unit tests pass.
- `z_shaped_rooms_a_and_c_not_mutually_visible` contract test passes.
- ALL contract tests pass — zero failures. This is the primary acceptance gate. The full list:
  - `corridor_connected_rooms_are_mutually_visible`
  - `solid_wall_blocks_all_cross_room_visibility`
  - `l_shaped_corridor_rooms_see_corridor`
  - `long_corridor_middle_sees_both_ends`
  - `z_shaped_rooms_a_and_c_not_mutually_visible`
  - `spatial_stability_within_room`
  - `small_brush_in_corridor_does_not_block_visibility`
  - `z_shaped_rooms_a_sees_b_and_b_sees_c`
- Corridor visibility must not regress — subdivision should not over-fragment cells in open areas. If `corridor_connected_rooms_are_mutually_visible` or `l_shaped_corridor_rooms_see_corridor` break, the subdivision is too aggressive.
- Compiler produces valid .prl from `test_map_4.map` and the existing test maps.
- `cargo check` and `cargo test` — zero failures.

**Depends on:** Task 2.

---

### Task 4: Consolidate `grid_cells_to_clusters` (cleanup)

**Files:** `src/spatial_grid.rs`, `src/main.rs`, `src/visibility.rs`, `src/geometry.rs`, `src/pack.rs`

Move the shared `grid_cells_to_clusters` function into `spatial_grid.rs` as a single public function. Remove the 4 duplicated copies. Pure refactor — behavior identical.

**Acceptance criteria:**
- `grid_cells_to_clusters` exists in exactly one location.
- All tests pass with same results as before this task.
- `cargo check` and `cargo test` pass.

**Depends on:** Task 3 (do this last so the core feature work isn't complicated by refactoring).

---

## Execution Order

```
Task 1 (move + reorder) ──> Task 2 (classify + air retention) ──> Task 3 (subdivision) ──> Task 4 (cleanup)
```

Strictly sequential. Each task is testable and deployable on its own.

---

## Files Changed

| File | Action |
|------|--------|
| `src/voxel_grid.rs` | **Moved** from `src/visibility/voxel_grid.rs` |
| `src/spatial_grid.rs` | **Modified** — classification, voxel-aware assignment, subdivision |
| `src/main.rs` | **Modified** — `pub mod voxel_grid`, pipeline reorder, pass voxel grid to spatial grid |
| `src/visibility.rs` | **Modified** — accept `&VoxelGrid`, remove internal construction |
| `src/visibility/pvs.rs` | **Modified** — update import path |
| `src/geometry.rs` | **Modified** — handle zero-face clusters |
| `src/pack.rs` | **Modified** — handle zero-face clusters |

All paths relative to `postretro-level-compiler/`.

---

## Implementation Notes

1. **Classification threshold.** Start with exact 100%/0%. A cell with even one air voxel is not `Solid`. Conservative — boundary cells get subdivided rather than miscategorized. Relax only if subdivision produces too many sub-cells in practice.

2. **Binary split axis selection.** For a boundary cell, iterate voxels along each axis. Count solid-to-empty transitions. Split along the axis with the most transitions (that's where the wall is). If tied, prefer the axis where the cell is longest.

3. **Face reassignment during subdivision.** Faces whose centroids land in solid sub-cells go to the nearest air sub-cell. Faces sit on surfaces near solid/air boundaries — they should never be lost.

4. **Zero-face clusters downstream.** `face_start=N, face_count=0` must be valid in the .prl format. The engine already handles "nothing to draw" for a cluster (it just draws nothing). But verify that `extract_geometry` doesn't skip zero-face clusters in a way that shifts face indices for subsequent clusters.

5. **Air cluster count.** A large open room could have many air-only clusters (no faces, just bounds for camera containment). At 16 cells/axis, max 4096 total cells — manageable. Log the count so we can monitor. If it becomes a problem, adjacent air cells can be merged as a follow-up.
