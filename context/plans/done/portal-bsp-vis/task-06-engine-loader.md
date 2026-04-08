# Task 06: Engine Loader

**Crate:** `postretro`
**Files:** `src/prl.rs` · possibly `src/renderer.rs` or equivalent render path
**Depends on:** Task 05

---

## Context

The engine's PRL loader (`src/prl.rs`) currently loads the Geometry and ClusterVisibility sections and builds `LevelWorld` with `Vec<ClusterData>`. It finds the camera's cluster via a linear AABB scan and uses the cluster's PVS for culling.

After Task 05, the PRL file contains BSP tree nodes, BSP leaves, and leaf PVS. The loader must be rewritten to:
- Build a runtime BSP tree for O(log n) point-in-leaf lookup
- Load per-leaf face ranges for draw grouping
- Load per-leaf PVS bitsets for culling

The `ClusterData` type and the linear cluster scan are removed.

---

## What to Change

### `src/prl.rs` — Runtime types

Replace `ClusterData` and `LevelWorld::clusters: Vec<ClusterData>` with:

```rust
pub struct LevelWorld {
    pub vertices: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub face_meta: Vec<FaceMeta>,   // per-face: index_offset, index_count
    pub leaves: Vec<LeafData>,
    pub nodes: Vec<NodeData>,
    pub root: BspChild,             // root node or leaf of the BSP tree
    pub has_pvs: bool,
}

pub struct LeafData {
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub face_start: u32,
    pub face_count: u32,
    pub pvs: Vec<bool>,             // decompressed: pvs[i] = leaf i is visible
    pub is_solid: bool,
}

pub struct NodeData {
    pub plane_normal: Vec3,
    pub plane_distance: f32,
    pub front: BspChild,
    pub back: BspChild,
}

pub enum BspChild {
    Node(usize),
    Leaf(usize),
}
```

### `src/prl.rs` — `find_leaf` via BSP descent

Replace the linear AABB scan with BSP tree descent:

```rust
pub fn find_leaf(&self, position: Vec3) -> usize {
    // Walk from root, at each node choose front or back based on plane side
    // until a leaf is reached. Return the leaf index.
}
```

Fallback behavior:
- If position is on the plane (within epsilon), choose front.
- If the tree is empty (no nodes), return leaf 0.
- If BSP descent lands in a solid leaf (camera clipped into geometry), log a warning and fall back to all-visible rendering (draw all leaves). Do not attempt to find the "nearest empty leaf" — this adds complexity for a rare edge case. The all-visible fallback is correct and simple.

### `src/prl.rs` — PVS lookup

After `find_leaf(camera_position)`:
- If the camera is in a solid leaf, the `find_leaf` fallback (above) already handles this — all leaves are drawn.
- Otherwise, use `leaf.pvs` to determine which other leaves to draw.

### Renderer integration

Update wherever the engine iterates over clusters for culling and drawing to iterate over leaves instead. The interface contract is the same — a `Vec<bool>` of visible leaves, a face range per leaf — only the type names change.

### Winding order

Verify that the coordinate transform applied in Task 00 (`quake_to_engine`: negates Y, swapping it into Z) produces correct face winding. If back-face culling is enabled and faces appear inverted, reverse the index winding order in the geometry section output (swap indices 1 and 2 of each triangle in the fan triangulation in `geometry.rs`).

---

## Acceptance Criteria

- `cargo check` and `cargo test` pass.
- `cargo run -p postretro -- assets/maps/test.prl` — engine loads the map without error. Camera navigates the level.
- PVS culling is active: log the visible leaf count per frame for 1 second, confirm it is less than total leaf count when inside the level.
- Point-in-leaf uses BSP descent — verify by confirming `find_leaf` is called (log on first call, then disable logging).
- No references to `ClusterData` remain in the engine.
- `qbsp` crate is still a dependency at this point (removal in Task 07). The BSP map loading path via qbsp can remain untouched.
- `cargo test` — zero failures.
