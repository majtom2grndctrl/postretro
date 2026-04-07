# Task 07: Cleanup

**Crates:** all three (`postretro`, `postretro-level-compiler`, `postretro-level-format`)
**Depends on:** Task 06

---

## Context

All new code is in place and working. This task removes the scaffolding: voxel and cluster code in the compiler, the qbsp engine dependency, and the glam version pin that only existed for qbsp compatibility.

---

## What to Remove

### `postretro-level-compiler`

Delete these files entirely:
- `src/voxel_grid.rs`
- `src/spatial_grid.rs`
- `src/partition/cluster.rs`
- `src/visibility/pvs.rs`
- `src/spatial_grid_test_fixtures.rs`
- `src/visibility_test_fixtures.rs` — if it contains fixtures for the old voxel PVS only; retain if any fixtures are still used by portal vis tests

Remove `mod` declarations for all deleted files from their parent modules. Remove any `use` imports referencing them. Confirm `cargo check` is clean.

### `postretro` — remove qbsp dependency

**Retain qbsp.** The engine still accepts `.bsp` files on the command line (`cargo run -p postretro -- assets/maps/test.bsp` is a documented workflow in CLAUDE.md). Do not remove qbsp from `postretro/Cargo.toml` and do not delete the BSP loading code. The `.bsp` path coexists with the `.prl` path.

### Workspace `Cargo.toml` — glam pin

Since qbsp is retained, leave the `glam = "0.30"` pin as-is. It exists for qbsp type compatibility.

### `postretro-level-format`

Remove `ClusterVisibilitySection` and `VisibilityConfidenceSection` types (in `src/visibility.rs` and `src/confidence.rs`). Retain the `rle_compress` / `rle_decompress` functions — they are used by the new `LeafPvsSection`. Mark the retired section ID constants (`SECTION_CLUSTER_VISIBILITY`, `SECTION_VISIBILITY_CONFIDENCE`) with a doc comment noting retirement.

---

## Acceptance Criteria

- `cargo fmt --check && cargo clippy -- -D warnings && cargo test` — all pass, zero warnings.
- No files named `voxel_grid.rs`, `spatial_grid.rs`, or `cluster.rs` exist in the compiler.
- No `mod voxel_grid`, `mod spatial_grid`, or `mod cluster` declarations anywhere.
- No references to `VoxelGrid`, `SpatialGrid`, or `Cluster` (the old partition cluster, not an unrelated type) anywhere in the workspace.
- `ClusterVisibilitySection` and `VisibilityConfidenceSection` are removed from `postretro-level-format` public API.
- `cargo run -p postretro -- assets/maps/test.prl` still loads and renders correctly.
- Task completion report notes whether qbsp was removed or retained, and why.
