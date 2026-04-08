# Task 04: Portal Vis

**Crate:** `postretro-level-compiler`
**New file:** `src/visibility/portal_vis.rs`
**Removes:** `src/visibility/pvs.rs` · `src/voxel_grid.rs` · `src/visibility.rs` (rewrite)
**Depends on:** Task 03

---

## Context

The voxel ray-cast PVS (`visibility/pvs.rs`) is replaced with portal flood-fill vis. Given the portal graph from Task 03, each empty leaf's PVS is computed by flood-filling through portals reachable from that leaf.

This is a conservative implementation — it does not implement antipenumbra clipping or angular sets. A leaf L' is potentially visible from leaf L if there exists any sequence of portals connecting L to L'. This is equivalent to graph reachability on the portal graph.

**Important limitation:** Pure flood-fill computes connected components. If all empty leaves are reachable from each other through portals (typical for a normal level with connected rooms), the PVS will be all-visible — every empty leaf sees every other empty leaf. This provides zero culling benefit. This is acceptable for the initial implementation because:
1. It is correct (never culls visible geometry).
2. It is implementable without weeks of geometric precision work.
3. It validates the full pipeline (BSP → portals → PVS → PRL → engine loader).
4. It is upgradeable to tighter bounds (antipenumbra, angular sets, or depth-limited flood) without changing the PRL format.

A future optimization pass should implement at minimum 1-hop or 2-hop portal visibility (only mark leaves reachable within N portal traversals) or full antipenumbra clipping to produce a PVS with actual culling value.

---

## What to Build

### `src/visibility/portal_vis.rs`

**Build a portal adjacency graph:**

```rust
// For each empty leaf, which portals touch it?
// A portal touches a leaf if front_leaf == leaf_idx OR back_leaf == leaf_idx.
fn leaf_portals(portals: &[Portal], leaf_count: usize) -> Vec<Vec<usize>>
// Returns: leaf_portals[leaf_idx] = list of portal indices touching that leaf
```

**Per-leaf BFS:**

```rust
pub fn compute_pvs(
    portals: &[Portal],
    leaf_count: usize,
    solid: &[bool],  // solid[i] = BspTree::leaves[i].is_solid
) -> Vec<Vec<bool>>
// Returns: pvs[leaf_idx][other_leaf_idx] = true if potentially visible
```

For each non-solid leaf L:
1. Initialize a boolean visited set of size `leaf_count`, all false.
2. BFS/flood from L: mark L visible. For each portal touching the current leaf, if the portal connects to a non-solid neighbor leaf not yet visited, mark it visible and enqueue it.
3. A leaf is always visible to itself.
4. Solid leaves are never visible (they are not navigable space and should never be the camera's leaf).

**Parallelism:** Use `rayon::par_iter` to compute PVS for all non-solid leaves in parallel. Each leaf's BFS is independent.

**Output encoding:** The PVS is a flat `Vec<Vec<bool>>` — one `Vec<bool>` per leaf, length `leaf_count`. Convert to RLE-compressed bitsets for packing (same format as the retired `ClusterVisibilitySection` — see `postretro-level-format/src/visibility.rs` for the RLE codec, which is retained).

### `src/visibility.rs`

Rewrite as a thin orchestration module:
1. Call `generate_portals` (from Task 03 `portals.rs`).
2. Call `compute_pvs` with the portal list and leaf solid flags.
3. Return per-leaf PVS bitsets.

Remove all references to `VoxelGrid`, `SpatialGrid`, and `compute_pvs_raycast`.

---

## Acceptance Criteria

- `cargo check` and `cargo test -p postretro-level-compiler` pass. Add `rayon` as a dependency if not already present.
- Unit test: two leaves connected by one portal — each sees the other.
- Unit test: three leaves in a chain (A↔B↔C) — A sees B and C, C sees A and B.
- Unit test: two leaves with no connecting portal — neither sees the other.
- Unit test: a solid leaf is never marked visible from any other leaf.
- `prl-build assets/maps/test.map` logs: leaf count, portal count, per-leaf visible-leaf counts (min/max/average).
- `cargo test -p postretro-level-compiler` — zero failures.
- No references to `VoxelGrid` or `SpatialGrid` remain in the compiler.
