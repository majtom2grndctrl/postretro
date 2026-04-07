# Task 02: BSP Compiler Hardening

**Crate:** `postretro-level-compiler`
**Files:** `src/partition/bsp.rs` · `src/partition/types.rs` · `src/partition.rs` · `src/main.rs`
**Depends on:** Task 01

---

## Context

`partition/bsp.rs` already builds a BSP tree with nodes and leaves. The tree is currently used in `--bsp` mode as a prelude to cluster assignment — the cluster abstraction (`partition/cluster.rs`) groups leaves into `Cluster` structs, which then feed visibility. That clustering layer is being removed. BSP leaves become the primary visibility units directly.

Two things need hardening before portal generation can proceed:

**1. The BSP must be the only partition path.** The `--bsp` flag and spatial grid path go away. The compiler always builds a BSP tree.

**2. Empty-leaf topology must be correct.** Portal generation depends on `BspLeaf::is_solid` being reliable. A solid leaf should represent a convex region inside brush geometry. An empty leaf should represent navigable air. The solid/empty boundary is where portals live. If classification is wrong, portal extraction produces nonsense.

---

## What to Build / Change

### `src/main.rs` and `src/partition.rs`

Remove the `--bsp` CLI flag and the spatial grid path. The compiler always runs BSP. `PartitionResult` still returns `BspTree` and `Vec<Face>`. Remove `clusters: Vec<Cluster>` from `PartitionResult` — the output of partitioning is now just the BSP tree and post-split faces.

### `src/partition/types.rs`

Remove the `Cluster` struct. It is no longer used after this task.

### `src/partition/bsp.rs` — Verify and harden `classify_leaf_solidity`

The existing implementation tests the centroid of a leaf's face geometry against brush volumes. This works for wall/floor faces (whose centroid sits inside the brush) but may misclassify thin or edge-case leaves.

Improve reliability: for each leaf, test multiple candidate points — the face centroid is the primary test, but also test each individual face's centroid. A leaf is solid if **any** candidate point is inside a brush volume. A leaf with no faces should be classified as solid (empty space always has bounding faces). Note: the current code at `bsp.rs` skips faceless leaves (`if leaf.face_indices.is_empty() { continue; }`), leaving them at the default `is_solid: false`. This must be changed to explicitly set `is_solid = true` for faceless leaves — flipping this default is intentional and required for correct portal generation.

### `src/partition/bsp.rs` — `MAX_LEAF_FACES`

Keep `MAX_LEAF_FACES` at a small value (2–4). Do **not** lower it to 1. Single-face leaves cause a BSP explosion (dramatically more nodes and leaves) and produce many tiny portal polygons prone to numerical precision issues. The standard BSP portal algorithm works correctly on arbitrarily-sized convex leaf regions — single-face leaves are not required. The existing test `build_bsp_tree_opposing_faces` asserts behavior based on `MAX_LEAF_FACES >= 2`; lowering to 1 would break it without benefit. If the current value of 4 causes issues during portal generation (Task 03), revisit then.

### Compiler pipeline in `src/main.rs`

Remove all references to `voxel_grid.rs`, `spatial_grid.rs`, and `partition/cluster.rs` from the pipeline. The pipeline is now:

```
parse → build_bsp_tree → classify_leaf_solidity → [portal + vis — Tasks 03, 04] → geometry → pack
```

For now (until Tasks 03/04), wire a pass-through: all empty leaves are mutually visible (degenerate PVS — all bits set). This keeps the compiler producing valid .prl files throughout the plan.

---

## Acceptance Criteria

- `cargo check` and `cargo test -p postretro-level-compiler` pass.
- `prl-build assets/maps/test.map -o assets/maps/test.prl` succeeds. Logs show leaf count, how many are solid, how many are empty.
- No `--bsp` flag. BSP is always used.
- No `Cluster` type anywhere in the compiler.
- No imports of `voxel_grid`, `spatial_grid`, or `cluster` modules in `main.rs` or `partition.rs`.
- Test: a simple two-room map (two box rooms connected by a corridor) produces at least 2 empty leaves and at least 1 solid leaf.
- Existing BSP unit tests in `partition/bsp.rs` pass unchanged (or adapted if `MAX_LEAF_FACES` change causes structural differences).
