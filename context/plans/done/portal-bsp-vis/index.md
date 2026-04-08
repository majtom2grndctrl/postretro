# Portal-Based BSP Vis

> **Status:** tasks 00–07 complete; task 08 ready for implementation
> **Depends on:** PRL Phase 1 (complete). Compiler produces .prl files. Engine loads them.
> **Related:** `context/lib/build_pipeline.md` · `context/lib/development_guide.md` · `context/lib/testing_guide.md`

---

## Goal

Replace the voxel ray-cast PVS with portal-based BSP visibility. The compiler builds a BSP tree, extracts portal polygons at splitting planes, and stores them in the PRL file. At runtime, the engine walks the portal graph with frustum clipping for per-frame visibility that naturally handles corners and narrow apertures. A precomputed flood-fill PVS mode is retained as a compiler option (`--pvs`). The PRL format gains BSP tree, leaf, and portal sections. The voxel grid, spatial grid, and cluster abstraction are removed entirely.

---

## Scope

### In scope

- Coordinate transform in the compiler: Quake Z-up → engine Y-up at parse time
- BSP compiler hardening: solid/empty leaf classification, correct empty-leaf topology
- Portal generation: clip splitting-plane polygons against ancestor planes to produce portal geometry
- Portal vis (precomputed): per-empty-leaf PVS bitsets via portal flood-fill, retained as `--pvs` compiler mode
- New PRL sections: BSP nodes, BSP leaves, leaf PVS bitsets, portal geometry
- Engine loader rewrite: BSP tree descent for point-in-leaf, leaf-based PVS culling
- Runtime portal traversal: per-frame frustum-clipped portal walk as default visibility mode
- Cleanup: remove voxel grid, spatial grid, cluster types (qbsp retained — `.bsp` loading still active)

### Out of scope

- Antipenumbra/angular-set precomputed vis (replaced by runtime portal traversal in Task 08)
- Texture sections, lighting sections, or nav mesh sections
- Incremental recompilation
- Dynamic portal open/close at runtime (future work enabled by Task 08's architecture)

---

## Shared Context

All tasks operate within the `postretro` workspace (three crates: `postretro`, `postretro-level-format`, `postretro-level-compiler`).

**What is being replaced:** The compiler's voxel grid PVS (`voxel_grid.rs`, `spatial_grid.rs`, `visibility/pvs.rs`) and the cluster abstraction (`partition/cluster.rs`). The PRL `ClusterVisibility` section (ID 2) and `VisibilityConfidence` section (ID 11) are retired.

**What is being kept:** The BSP tree construction in `partition/bsp.rs` and `partition/types.rs`. The geometry extraction in `geometry.rs`. The PRL container format (header, section table) and geometry section (ID 1). The parse stage in `parse.rs` gains only a coordinate transform.

**Key compiler types (pre-existing):**
- `Face` — convex polygon with vertices, normal, distance, texture name. Lives in `map_data.rs`.
- `BrushVolume` / `BrushPlane` — convex brush hull for solid/empty classification. Lives in `map_data.rs`.
- `BspTree` — arena of `BspNode` (splitting plane + front/back children) and `BspLeaf` (face indices, bounds, solid flag). Lives in `partition/types.rs`.
- `build_bsp_tree(faces) -> (BspTree, Vec<Face>)` — builds the tree, in `partition/bsp.rs`.
- `classify_leaf_solidity(tree, faces, brushes)` — marks leaves solid/empty, in `partition/bsp.rs`.

**Coordinate convention (engine-native, Y-up):**
- Input from `.map`: Quake Z-up (X=right, Y=forward, Z=up).
- Engine: Y-up, right-handed (X=right, Y=up, Z=back).
- Transform: `Vec3::new(-v.y, v.z, -v.x)`. Applied to vertex positions, face normals, brush plane normals. Plane distances are scalars — unchanged. This is the same swizzle used in `postretro/src/bsp.rs`.
- After Task 00, all data exiting `parse.rs` is in engine coordinates. Every subsequent task assumes engine coordinates throughout.

---

## Task List

| ID | Task | File | Depends on |
|----|------|------|------------|
| 00 | Coordinate transform | `task-00-coord-transform.md` | — |
| 01 | Build pipeline doc update | `task-01-build-pipeline-doc.md` | 00 |
| 02 | BSP compiler hardening | `task-02-bsp-hardening.md` | 01 |
| 03 | Portal generation | `task-03-portal-generation.md` | 02 |
| 04 | Portal vis | `task-04-portal-vis.md` | 03 |
| 05 | New PRL sections | `task-05-prl-sections.md` | 04 |
| 06 | Engine loader | `task-06-engine-loader.md` | 05 |
| 07 | Cleanup | `task-07-cleanup.md` | 06 |
| 08 | Runtime portal traversal | `task-08-runtime-portal-vis.md` | 07 |

---

## Execution Order

```
T00 → T01 → T02 → T03 → T04 → T05 → T06 → T07 → T08
```

Strictly sequential. Each task's output is the next task's input.

| Phase | Task | Notes |
|-------|------|-------|
| 0 | 00 | Coordinate transform. Prerequisite for all subsequent work. |
| 1 | 01 | Update context docs to reflect new pipeline. Agents in later phases read these. |
| 2 | 02 | BSP hardening. Correctness foundation for portal extraction. |
| 3 | 03 | Portal generation. New compiler stage. |
| 4 | 04 | Portal vis. Replaces pvs.rs. |
| 5 | 05 | PRL format additions. New sections in postretro-level-format. |
| 6 | 06 | Engine loader rewrite. BSP descent, leaf PVS culling. |
| 7 | 07 | Remove voxel/cluster code, drop qbsp, relax glam pin. |
| 8 | 08 | Runtime portal traversal with frustum clipping. Default mode; PVS retained via `--pvs`. |

---

## Acceptance Criteria

1. `cargo test -p postretro-level-compiler` — zero failures.
2. `cargo test -p postretro-level-format` — zero failures.
3. `cargo test` (workspace) — zero failures.
4. `prl-build assets/maps/test.map -o assets/maps/test.prl` (default mode) — compiles without error, produces .prl with Portals section (15). Logs show leaf count, portal count, and per-leaf visibility stats.
5. `prl-build assets/maps/test.map --pvs -o assets/maps/test.prl` — produces .prl with LeafPvs section (14) instead of Portals section.
6. `cargo run -p postretro -- assets/maps/test.prl` — engine loads and renders the map with runtime portal traversal. Camera navigates the level. Visibility culling is active (fewer draw calls than total leaf count when inside the level). Geometry around corners is culled.
7. `cargo fmt --check && cargo clippy -- -D warnings` — clean.
8. No references to `VoxelGrid`, `SpatialGrid`, or `Cluster` remain in the compiler.
9. `qbsp` remains a dependency of `postretro` (`.bsp` loading path is retained).

---

## What Carries Forward

| Output | Consumers |
|--------|-----------|
| BSP tree in .prl | Engine point-in-leaf lookup; future collision, audio zone resolution |
| Leaf PVS bitsets | Renderer visibility culling; future audio propagation |
| Engine-native coordinates from parse.rs | All compiler stages; all future PRL sections |
| Portal flood-fill vis (`--pvs`) | Fallback/comparison mode; retained for levels where compile-time PVS is preferred |
| Runtime portal traversal (default) | Per-frame frustum-clipped portal walk; enables dynamic geometry in future |
