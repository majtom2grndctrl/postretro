# Adaptive Voxel Density

> **Status:** draft.
> **Depends on:** Voxel-Aware Spatial Grid (complete). VoxelGrid + spatial grid pipeline working.
> **Related:** `context/lib/build_pipeline.md`, `context/lib/development_guide.md`, `context/lib/testing_guide.md`

---

## Goal

Replace the uniform-resolution voxel grid with an adaptive octree that uses fine voxels near brush surfaces and coarse voxels in open air and deep solid. This enables finer resolution where accuracy matters (thin walls, detailed geometry) without blowing up memory on large maps.

---

## Scope

### In scope

- Octree data structure for adaptive solid/empty storage
- Bottom-up construction from the existing sealed flat grid
- Configurable fine and coarse voxel sizes (defaults: 4.0 fine, 16.0 coarse)
- Hierarchical ray marching through the octree (replaces flat-grid 3D-DDA)
- Point-in-solid queries through octree traversal
- Pipeline integration: voxelize (fine, flat) -> seal exterior -> build octree -> spatial grid + PVS use octree

### Out of scope

- Changes to seal_exterior (stays on the flat grid, runs before octree construction)
- Changes to VoxelGrid construction or sealing logic
- .prl format changes (octree is compile-time only)
- Engine changes
- GPU-friendly octree representations (compile-time tool, not real-time)
- Spatial grid algorithm changes (it consumes the same query API)

---

## Background: Algorithm Research

**Data structure: Sparse Voxel Octree (SVO).** Each node is either a leaf (uniformly solid or empty) or a branch with 8 children. Uniform regions collapse to a single node regardless of depth. Memory is proportional to surface area, not volume. See Laine & Karras 2010 (NVIDIA ESVO). A 64-tree (4x4x4 branching, 64-bit bitmasks) is faster but locks resolution jumps to 4x — less flexible for tuning. Classic 2x2x2 octree gives 2x jumps between levels, better for experimentation.

**Construction: Bottom-up merging.** Start with the fine uniform grid, merge uniform 2x2x2 blocks into parent nodes, recurse upward. O(N) scan. Described by Baert et al. for out-of-core SVO construction. Ideal because we already produce a sealed flat grid — the merge pass adds minimal cost.

**Ray marching: Revelles parametric octree traversal (2000).** Top-down: compute which child the ray enters, test it, advance to next sibling via parametric comparison. Coarse empty leaves are skipped in one step (the ray jumps to the far side of the node). O(depth) per node transition vs. O(1) per step in flat DDA, but far fewer steps through open space. Canonical reference implementation by Jeroen Baert.

**Neighbor finding: Samet (1989).** Ascend to common ancestor, descend to target neighbor. O(depth) worst case. Required if flood fill ever moves to the octree, but not needed for this plan — seal stays on the flat grid.

---

## Shared Context

All tasks operate within `postretro-level-compiler`. Key types:

- `VoxelGrid` (`src/voxel_grid.rs`) — flat 3D bitmap with `is_solid(x, y, z)`, `is_point_solid(pos)`, `seal_exterior(seed)`. Stays as-is for construction and sealing.
- `OctreeGrid` (new, `src/octree_grid.rs`) — adaptive replacement for downstream consumers. Built from a sealed `VoxelGrid`.
- `GridCell` / `CellType` / `SpatialGridResult` (`src/spatial_grid.rs`) — spatial grid system that queries voxel data for cell classification.
- `ray_blocked_by_voxels` (`src/visibility/pvs.rs`) — current flat-grid 3D-DDA. Replaced by octree traversal.

### Pipeline after this plan

```
parse .map
  -> voxelize brushes (fine flat grid, existing VoxelGrid)
  -> seal exterior (existing flood fill on flat grid)
  -> build octree (new: merge flat grid into OctreeGrid)
  -> spatial grid (uses OctreeGrid for cell classification)
  -> PVS (uses OctreeGrid for ray marching + point queries)
  -> geometry -> pack .prl
```

The flat VoxelGrid is temporary — constructed, sealed, consumed by octree construction, then dropped. Both grids exist simultaneously during `from_voxel_grid`; peak memory is roughly 2x the flat grid. This is a transient cost during compilation, not a runtime concern.

### Configurable resolution

Two constants control density (easy to change for experimentation):

- `FINE_VOXEL_SIZE: f32` — finest resolution, used near brush surfaces. Default 4.0.
- `COARSE_VOXEL_SIZE: f32` — coarsest resolution, used in uniform regions. Default 16.0.

Octree depth is derived: `depth = log2(coarse / fine)`. With 4.0/16.0 defaults, depth = 2 (levels at 4, 8, 16 units). With 2.0/16.0, depth = 3 (levels at 2, 4, 8, 16). Constraint: coarse must be a power-of-two multiple of fine.

**Auto-coarsen interaction.** The flat grid may auto-coarsen the voxel size on large maps (e.g., 4.0 -> 8.0). The octree builder uses the flat grid's *actual* `voxel_size`, not the `FINE_VOXEL_SIZE` constant, to compute depth. If `grid.voxel_size >= COARSE_VOXEL_SIZE`, the octree is a single-level wrapper (depth 0) and provides no adaptive benefit — log a warning. If `grid.voxel_size` is between fine and coarse, derive depth from the actual ratio and log that the effective fine resolution differs from the configured default.

### Cubic octree invariant

A standard octree subdivides a cube into 8 equal cubes. Non-cubic worlds are handled by **padding to a cube**: the root node's extent is the largest axis of the world bounds, rounded up to the next multiple of `coarse_voxel_size`. All three axes use this same extent. Padding voxels beyond the original world bounds are treated as solid (sealed exterior). The wasted volume is in uniform solid regions that collapse to single leaf nodes — negligible memory cost.

When reading from the source flat grid during construction, coordinates beyond the flat grid's resolution are treated as solid (the padding-as-sealed-exterior rule). This ensures rays cannot escape around the octree boundary.

---

## Tasks

### Task 1: Octree Data Structure + Construction

**New file:** `postretro-level-compiler/src/octree_grid.rs`

Build the octree type and bottom-up merge from a sealed flat grid.

**What to build:**

- `OctreeNode` enum:
  - `Leaf(bool)` — uniformly solid (true) or empty (false)
  - `Branch(Box<[OctreeNode; 8]>)` — 8 children in Morton order (x varies fastest, then y, then z)
- `OctreeGrid` struct:
  - `bounds: Aabb` — world-space bounding box (padded to align with coarse grid)
  - `fine_voxel_size: f32` — finest resolution
  - `fine_resolution: [usize; 3]` — resolution at finest level
  - `depth: usize` — number of octree levels above leaves
  - `root: OctreeNode` — root of the tree
- `OctreeGrid::from_voxel_grid(grid: &VoxelGrid, coarse_voxel_size: f32) -> Self`:
  1. Read `grid.voxel_size` (the *actual* fine size, which may differ from `FINE_VOXEL_SIZE` if auto-coarsen fired). Compute depth from `log2(coarse / actual_fine)`. If `actual_fine >= coarse`, depth = 0 — log a warning that the octree provides no adaptive benefit. Validate that the ratio is a power of two; if not, round depth down and log the effective coarse size.
  2. Compute cubic extent: take the largest axis of the flat grid's world bounds, round up to the next multiple of `actual_fine * 2^depth`. All three axes use this same extent. This guarantees the root is a cube that subdivides cleanly.
  3. Bottom-up merge: read the flat grid at fine resolution. **Coordinates beyond the flat grid's resolution are treated as solid** (sealed exterior invariant). Group 2x2x2 blocks of leaves. If all 8 children are `Leaf(same_value)`, merge into `Leaf(same_value)`. Otherwise, `Branch`. Repeat at each level up to root.
  4. Log octree stats: actual fine/coarse sizes, depth, node count by level, total nodes, memory estimate, compression ratio vs flat grid.
- `is_solid(x: usize, y: usize, z: usize) -> bool` — traverse from root to leaf using fine-resolution coordinates. At each level, compute which of 8 children contains (x, y, z). If the node is a `Leaf`, return its value. Out-of-bounds (beyond cubic extent) returns false.
- `is_point_solid(world_pos: Vec3) -> bool` — convert world position to fine-grid coordinates, delegate to `is_solid`. Out-of-bounds returns false. Note: points in the cubic padding (beyond original world bounds but within octree extent) return solid — this is correct, as they represent sealed exterior.

**Constants (module-level, easy to change):**

- `pub const FINE_VOXEL_SIZE: f32 = 4.0;`
- `pub const COARSE_VOXEL_SIZE: f32 = 16.0;`

**Acceptance criteria:**

- Unit test: empty flat grid produces single `Leaf(false)` root.
- Unit test: uniform solid flat grid produces single `Leaf(true)` root.
- Unit test: box brush in center — `is_solid` and `is_point_solid` match flat grid for all test points (sample brush interior, exterior, and boundary).
- Unit test: hollow room geometry — interior air is empty, walls are solid, exterior (sealed) is solid. Verify octree queries match flat grid.
- Unit test: compression ratio — for a hollow room, octree node count is significantly less than flat grid voxel count (at least 10x fewer nodes).
- Unit test: cubic padding — a non-cubic world (e.g., 256x64x128 fine voxels) produces an octree with equal extent on all axes. Points in the padding region return solid.
- Unit test: padding-as-solid — coordinates beyond the flat grid's resolution are solid in the octree, not empty.
- Unit test: configuring different fine/coarse ratios (e.g., 2.0/16.0) produces the correct depth.
- Unit test: auto-coarsened flat grid (actual voxel_size > FINE_VOXEL_SIZE) produces an octree with reduced depth derived from the actual ratio.
- `cargo check` and `cargo test` pass.

**Depends on:** nothing.

---

### Task 2: Hierarchical Ray Marching

**Modified file:** `postretro-level-compiler/src/visibility/pvs.rs`

Replace flat-grid 3D-DDA with Revelles-style parametric octree traversal.

**What to build:**

- `ray_blocked_by_octree(grid: &OctreeGrid, start: Vec3, end: Vec3) -> bool`:
  1. If `start` is in solid, return true immediately.
  2. Convert ray to parametric form: compute t-values where the ray enters and exits the root node's bounding box. If ray misses the root, return false.
  3. Recursive traversal: at each node:
     - If `Leaf(true)`: ray is blocked.
     - If `Leaf(false)`: ray is unblocked through this node (skip to exit t-value).
     - If `Branch`: determine which child the ray enters first. Process children in ray-order (first-to-last along ray direction). For each child the ray passes through, recurse. If any child blocks, return true. Advance to next child via parametric stepping.
  4. The "first child" and "next child" logic uses the ray's sign bits to mirror the octree so traversal always proceeds in positive direction (Revelles' mirroring trick).

  **Numerical edge cases (must handle correctly):**
  - Axis-aligned rays: one or two direction components are zero. Use large sentinel t-values (e.g., `f32::MAX`) for zero-component axes to avoid division by zero.
  - Rays on node boundaries: a ray exactly on the boundary between two children must not be skipped or double-counted. Use the same epsilon tie-breaking strategy as the existing DDA.
  - Very short rays (start ~= end): return false (consistent with existing DDA behavior).
  - Rays starting inside the octree vs. outside: both must work. Clamp entry t-value to 0 for rays starting inside.

- `filter_solid_samples_octree(grid: &OctreeGrid, samples: &[Vec3]) -> Vec<Vec3>` — same as current `filter_solid_samples` but using octree `is_point_solid`.

**Acceptance criteria:**

- Unit test: ray through all-empty octree returns unblocked.
- Unit test: ray into solid octree returns blocked.
- Unit test: ray that passes through a coarse empty region returns unblocked.
- Unit test: ray that clips the corner of a solid region at fine resolution returns blocked.
- Unit test: ray starting in solid returns blocked.
- Unit test: diagonal ray through hollow room — unblocked inside, blocked through walls.
- Unit test: axis-aligned ray (e.g., along +X only) through empty space returns unblocked.
- Unit test: axis-aligned ray through a solid wall returns blocked.
- Unit test: very short ray (length < epsilon) returns unblocked.
- Equivalence test: for the hollow-room and box-brush geometries, `ray_blocked_by_octree` returns the same result as `ray_blocked_by_voxels` for a set of 100+ random ray pairs (include axis-aligned, diagonal, and near-boundary rays).
- `cargo check` and `cargo test` pass.

**Depends on:** Task 1.

---

### Task 3: Integration

**Modified files:** `src/main.rs`, `src/spatial_grid.rs`, `src/visibility.rs`, `src/visibility/pvs.rs`

Wire the octree into the pipeline. Replace `&VoxelGrid` with `&OctreeGrid` in downstream consumers.

**What to change:**

- **`main.rs`:**
  1. Add `pub mod octree_grid;` module declaration.
  2. After `seal_exterior`, build octree: `OctreeGrid::from_voxel_grid(&voxel_grid, COARSE_VOXEL_SIZE)`.
  3. Drop `voxel_grid` (free flat grid memory).
  4. Pass `&octree_grid` to `assign_to_grid` and `compute_visibility`.

- **`spatial_grid.rs`:**
  1. Change `use crate::voxel_grid::VoxelGrid` to `use crate::octree_grid::OctreeGrid`.
  2. Update `classify_cell`, `analyze_cell_axes`, `subdivide_boundary_cell`, `shrink_to_air_extent`, and `assign_to_grid` to accept `&OctreeGrid` instead of `&VoxelGrid`. The `Option` wrapper on `assign_to_grid`'s voxel parameter is dropped — the octree is now always available at this point in the pipeline.
  3. Existing voxel iteration logic (looping over `is_solid(ix, iy, iz)`) works unchanged — octree exposes the same query at fine resolution. Access `.fine_voxel_size` instead of `.voxel_size`, `.bounds` stays the same, `.fine_resolution` instead of `.resolution`.

- **`visibility.rs`:**
  1. Update `compute_visibility` to accept `&OctreeGrid` instead of `&VoxelGrid`.
  2. Pass through to `compute_pvs_raycast`.

- **`visibility/pvs.rs`:**
  1. Update `compute_pvs_raycast` to accept `&OctreeGrid`.
  2. Replace calls to `ray_blocked_by_voxels` with `ray_blocked_by_octree`.
  3. Replace `filter_solid_samples` with `filter_solid_samples_octree`.
  4. `generate_cluster_samples` is unchanged (it doesn't use voxel data).
  5. Delete `ray_blocked_by_voxels` and `filter_solid_samples` (fully replaced). Delete their unit tests. The equivalence tests in Task 2 verify the replacement is correct.

- **Test helpers:** Update `VoxelGrid::from_brushes` calls in test helpers across `spatial_grid.rs`, `visibility.rs`, `pvs.rs`, and `pack.rs`. Each helper builds the flat grid, seals it, then constructs an `OctreeGrid` for downstream use. Add a shared test utility function (e.g., `test_octree_from_brushes`) to avoid repeating this boilerplate.

- **Context file update:** After integration is complete, update `context/lib/build_pipeline.md` §PRL Compilation to reflect the octree construction step in the compiler pipeline.

**Acceptance criteria:**

- ALL contract tests pass — zero failures:
  - `corridor_connected_rooms_are_mutually_visible`
  - `solid_wall_blocks_all_cross_room_visibility`
  - `l_shaped_corridor_rooms_see_corridor`
  - `long_corridor_middle_sees_both_ends`
  - `z_shaped_rooms_a_and_c_not_mutually_visible`
  - `spatial_stability_within_room`
  - `small_brush_in_corridor_does_not_block_visibility`
  - `z_shaped_rooms_a_sees_b_and_b_sees_c`
- Compiler produces valid .prl from `test_map_4.map` and other test maps.
- Octree memory usage is logged and is less than flat grid equivalent for test maps.
- Performance: log wall-clock time for PVS computation. Regression > 2x vs. previous flat-grid baseline on test maps should be investigated and reported to the user.
- No dead code: `ray_blocked_by_voxels` and `filter_solid_samples` are removed.
- `cargo check` and `cargo test` — zero failures.
- `cargo clippy` — no new warnings.

**Depends on:** Task 2.

---

## Sequencing

```
Task 1 (octree structure + construction)
  -> Task 2 (hierarchical DDA)
  -> Task 3 (integration)
```

Strictly sequential. Task 1 is testable standalone. Task 2 needs the octree type. Task 3 needs both.

---

## Files Changed

| File | Action |
|------|--------|
| `src/octree_grid.rs` | **New** — octree data structure, construction, queries |
| `src/main.rs` | **Modified** — `pub mod octree_grid`, build octree after seal, pass to downstream |
| `src/spatial_grid.rs` | **Modified** — accept `&OctreeGrid`, drop `Option` wrapper, update field names |
| `src/visibility.rs` | **Modified** — accept `&OctreeGrid`, pass through |
| `src/visibility/pvs.rs` | **Modified** — octree DDA, octree sample filter, accept `&OctreeGrid`, delete old flat-grid functions |
| `context/lib/build_pipeline.md` | **Modified** — add octree construction step to PRL compiler pipeline |

All code paths relative to `postretro-level-compiler/`.

---

## Implementation Notes

1. **Morton order for children.** Child index = `(bit_x) | (bit_y << 1) | (bit_z << 2)` where `bit_x = (x >> level) & 1`. This convention must be consistent between construction (Task 1) and ray traversal (Task 2). The Revelles paper uses a different convention (their Table 1); if adapting Revelles or Baert's reference code, translate child indices to match the construction order. Pick one convention and use it everywhere.

2. **Cubic padding.** The octree root covers a cube — largest world axis, rounded to the next multiple of `coarse_voxel_size`. Padding beyond the world bounds is solid (sealed exterior). During construction, flat grid coordinates beyond its resolution are read as solid. The padding merges cleanly into coarse solid leaf nodes — negligible memory cost.

3. **Revelles mirroring.** The parametric traversal mirrors the ray direction so it always proceeds in (+x, +y, +z). This reduces the child-ordering logic from 8 cases to 1 (with a mirror bitmask to flip child indices). Jeroen Baert's C++ implementation is a good reference. When adapting, verify the child index convention matches Note 1.

4. **Flat grid stays for construction.** The flat `VoxelGrid` is the "rasterizer" — it tests brush half-planes per voxel center and handles auto-coarsening for pathologically large maps. The octree is built from the rasterized result. Don't duplicate the rasterization logic.

5. **Field renaming for clarity.** `OctreeGrid` uses `fine_voxel_size` and `fine_resolution` to distinguish from the coarse levels. Consumers that currently read `.voxel_size` and `.resolution` update to the new names — a mechanical change. `fine_resolution` reflects the cubic padded extent (equal on all axes), not the original world bounds. `depth` is derived from the actual fine/coarse ratio, not stored independently — `depth = log2(fine_resolution[0] * fine_voxel_size / coarse_extent)`. However, storing `depth` as a field for convenience is fine as long as construction validates consistency.

6. **Future: region queries.** `spatial_grid.rs`'s `classify_cell` iterates all fine voxels in a cell's bounds. An `is_region_uniform(bounds) -> Option<bool>` method could short-circuit by checking the octree at the cell's scale. Deferred — correctness first, optimize if profiling shows it matters.

7. **Future: 64-tree upgrade.** If profiling shows octree traversal is slow, the 2x2x2 octree can be replaced with a 4x4x4 branching tree (64-bit bitmask per node) for fewer levels and faster traversal. The API stays the same. The 2x2x2 tree is chosen first because it supports any power-of-two fine/coarse ratio, not just 4x jumps.

---

## Research References

- Laine & Karras, "Efficient Sparse Voxel Octrees" (NVIDIA 2010)
- Revelles, Urena, Lastra, "An Efficient Parametric Algorithm for Octree Traversal" (WSCG 2000)
- Baert, Lagae, Dutre, "Out-of-Core Construction of Sparse Voxel Octrees" (HPG 2013)
- Samet, "Neighbor Finding in Octrees" (CVGIP 1989)
- Museth, "VDB: High-Resolution Sparse Volumes with Dynamic Topology" (SIGGRAPH 2013) — HDDA concept
- DubiousConst282, "Fast Voxel Ray Tracing Using Sparse 64-Trees" (2024 blog)
- Geidav, "Advanced Octrees: Neighbor Finding" (2017 blog)
