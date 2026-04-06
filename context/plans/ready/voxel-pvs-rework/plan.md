# Voxel-Based PVS Rework

> **Status:** ready for implementation.
> **Depends on:** PRL Phase 1 (complete). Compiler produces .prl files with spatial grid + ray-cast PVS.
> **Related:** `context/lib/build_pipeline.md`, `context/lib/development_guide.md`, `context/lib/testing_guide.md`
> **Note:** `context/lib/rendering_pipeline.md` is legacy while we experiment with new visibility solutions. Do not treat it as authoritative for PVS behavior.

---

## Goal

Replace the PVS internals with a voxel-based approach. Voxelize brush geometry into a 3D solid/empty bitmap at compile time. Use this bitmap for (a) rejecting sample points inside solid geometry and (b) 3D-DDA ray marching instead of per-brush slab testing. Fix the 2 failing contract tests without regressing the passing ones.

---

## Scope

### In scope

- Voxelizer: converts brush volumes into a 3D bitmap (solid/empty per voxel)
- Point-in-solid classification via voxel lookup
- 3D-DDA ray marching through the voxel grid (replaces slab method)
- Updated `compute_pvs_raycast` to use the voxel grid

### Out of scope

- .prl format changes (voxel grid is compile-time only)
- Engine changes
- Octree replacement of the uniform spatial grid (future optimization if needed)
- Pipeline shape changes

---

## Shared Context

All tasks operate within `postretro-level-compiler`. Key types:

- `BrushVolume` — convex hull defined by `BrushPlane { normal: Vec3, distance: f32 }`. Point inside when `dot(point, normal) - distance <= 0` for all planes.
- `Cluster` — spatial region with face assignments, AABB, used for PVS.
- `Face` — polygon with vertices, normal, texture info.
- Coordinate system: engine Y-up (converted from Quake Z-up during parsing).

---

## Tasks

### Task 1: Voxelizer

**New file:** `postretro-level-compiler/src/visibility/voxel_grid.rs`

Build a 3D bitmap from brush volumes. Each voxel is solid (inside any brush) or empty (air).

**What to build:**

- `VoxelGrid` struct: world-space AABB, resolution per axis `[usize; 3]`, flat bitset.
- `VoxelGrid::from_brushes(brush_volumes, world_bounds, voxel_size) -> VoxelGrid` — for each voxel, test center point against all brush half-planes. Mark solid if inside any brush. Accelerate with AABB pre-filter per brush.
- `is_solid(x, y, z) -> bool` — single voxel query.
- `is_point_solid(world_pos: Vec3) -> bool` — world-to-grid conversion, returns solid/empty. Out-of-bounds returns false.
- Default voxel size: 4 world units. Log grid dimensions and memory usage.

**Auto-coarsen rule:** If the grid at the requested voxel size would exceed `MAX_VOXELS` (512 per axis, ~16 MB bitmap), increase voxel size until the grid fits. The constructor handles this automatically and logs when coarsening occurs. This keeps memory bounded without requiring callers to reason about map size.

**Acceptance criteria:**

- Unit test: empty brush list produces all-empty grid.
- Unit test: single box brush produces solid voxels inside, empty outside.
- Unit test: `is_point_solid` returns true for brush center, false for known air.
- Unit test: two overlapping brushes — overlap region is solid.
- Unit test: out-of-bounds query returns false.
- `cargo check` and `cargo test` pass.

**Depends on:** nothing.

---

### Task 2: 3D-DDA Ray Marching + Sample Filtering

**Modified file:** `postretro-level-compiler/src/visibility/pvs.rs`

Replace slab-method ray casting with 3D-DDA through the voxel grid. Add solid-point rejection to sample generation.

**What to build:**

- `ray_blocked_by_voxels(grid, start, end) -> bool` — Amanatides & Woo 3D-DDA. Step through voxels along the ray; return true if any solid voxel is hit. Clip to grid bounds. Handle start-in-solid gracefully (return true).
- `filter_solid_samples(grid, samples) -> Vec<Vec3>` — remove sample points in solid voxels.

**What to change in `compute_pvs_raycast`:**

- Change signature: accept `&VoxelGrid` instead of `&[BrushVolume]` and `min_cell_dim`.
- Filter sample points through `filter_solid_samples` before pair testing.
- Replace inner `ray_vs_brush` loop with `ray_blocked_by_voxels`.
- Remove the brush size filtering logic (voxel grid handles this naturally).

**Acceptance criteria:**

- Unit test: DDA ray through empty grid returns unblocked.
- Unit test: DDA ray through solid region returns blocked.
- Unit test: DDA ray around a solid region returns unblocked.
- Unit test: ray starting in solid voxel returns blocked.
- Unit test: `filter_solid_samples` keeps air points, removes solid points.
- Existing `pvs.rs` unit tests adapted and passing.
- `cargo check` and `cargo test` pass.

**Depends on:** Task 1.

---

### Task 3: Integration

**Modified file:** `postretro-level-compiler/src/visibility.rs`

Wire the voxel grid into the visibility pipeline.

**What to change:**

- Add `mod voxel_grid;` declaration.
- In `compute_visibility`, before PVS computation:
  1. Compute world AABB from cluster bounds.
  2. Build `VoxelGrid::from_brushes(brush_volumes, &world_bounds, voxel_size)`.
  3. Log voxel grid stats.
- Update `compute_pvs_raycast` call to pass voxel grid.

**Acceptance criteria:**

- All contract tests pass (the 6 currently passing + the 2 currently failing).
- `cargo test -p postretro-level-compiler` — zero failures.
- Compiler produces valid .prl from test maps.
- `cargo check` and `cargo test` pass.

**Depends on:** Task 2.

---

## Execution Order

```
Task 1 (voxelizer) ──> Task 2 (3D-DDA) ──> Task 3 (integration)
```

Strictly sequential.

---

## Files Changed

| File | Action |
|------|--------|
| `src/visibility/voxel_grid.rs` | **New** |
| `src/visibility/pvs.rs` | **Modified** — replace ray casting, change signature |
| `src/visibility.rs` | **Modified** — build voxel grid, wire it in |

All paths relative to `postretro-level-compiler/`. No other files change.

---

## Implementation Notes

These are notes for the implementing agent, not open questions.

1. **Voxel resolution.** 4 world units is the default. This gives 4 voxels across a 16-unit wall (the thinnest in test maps). The auto-coarsen rule (Task 1) handles large maps automatically.

2. **`spatial_stability_within_room` may not be a voxel problem.** This test fails because a point falls outside all cluster AABBs — a spatial grid coverage issue, not a ray-casting issue. If it still fails after Task 3, investigate `grid_cells_to_clusters` filtering empty cells. Small fix in the spatial grid layer. Escalate to the user if unclear.

3. **3D-DDA numerical edge cases.** Rays exactly on voxel boundaries need epsilon tie-breaking. Well-documented in Amanatides & Woo. Test coverage should include axis-aligned and diagonal rays.

4. **`ray_vs_brush` disposition.** Delete the slab method and its tests. It's being fully replaced by voxel ray marching.
