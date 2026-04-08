# Task 03: Portal Generation

**Crate:** `postretro-level-compiler`
**New file:** `src/portals.rs`
**Depends on:** Task 02

---

## Context

Portals are the apertures between adjacent BSP leaves. Each internal BSP node has a splitting plane. That splitting plane, clipped to the convex region of space the node partitions, is a portal polygon connecting the leaves on either side. Portal generation walks the BSP tree and produces all such polygons.

Portals are compile-time only. They are used by the vis stage (Task 04) and then discarded — they are not stored in .prl.

The reference algorithm is the recursive portal distribution approach from ericw-tools (`qbsp/portals.c`), which is the modern, numerically robust fork of the original Quake VIS compiler. The key functions to study are `MakeNodePortal`, `AddPortalToNodes`, and `SplitNodePortals`. Fabien Sanglard's writeups on the original Quake source are useful background, but prefer ericw-tools for implementation details — it fixes precision issues in winding clipping that the original id Software code had.

---

## What to Build

### `src/portals.rs`

```rust
pub struct Portal {
    pub polygon: Vec<Vec3>,   // convex polygon in engine coordinates
    pub front_leaf: usize,    // index into BspTree::leaves
    pub back_leaf: usize,     // index into BspTree::leaves
}

pub fn generate_portals(tree: &BspTree) -> Vec<Portal>
```

**Algorithm — recursive portal distribution (ericw-tools approach):**

The algorithm has two phases: (1) generate a portal winding at each internal node, and (2) recursively distribute that winding through both subtrees to find all actual leaf pairs it connects.

**Phase 1 — Generate portal winding at each node:**

Walk the BSP tree recursively, maintaining a stack of clipping planes (the ancestor nodes' splitting planes, oriented relative to the traversal side). At each internal node:

1. Create an initial polygon (winding) — a large square (e.g., 16384 units per side) centered on the node's splitting plane, axis-aligned to the plane normal. To construct it: cross the plane normal with a reference axis (use +X if the normal is near-parallel to +Z, otherwise use +Z) to get the first basis vector, then cross that result with the normal to get the second basis vector. Normalize both, scale each to the desired half-extent, and form a quad from ±basis1 ±basis2, offset by `normal * distance` to center it on the plane. The reference-axis selection must avoid near-parallel normals — skipping this check produces a degenerate or zero-length first cross product.
2. Clip this polygon against all planes in the ancestor stack. Each clip discards the portion of the polygon on the wrong side of the ancestor plane. If the polygon becomes degenerate (< 3 vertices or area below a minimum threshold), discard — no portal at this node.
3. Pass the surviving winding to Phase 2 (distribute).
4. Recurse into both children, extending the ancestor plane stack.

When descending into the front child of a node, add the node's plane to the stack as-is (normal points to front = clipping preserves front side). When descending into the back child, add the negated plane (normal reversed, distance negated).

**Phase 2 — Distribute portal to leaf pairs:**

A portal at node N potentially connects every leaf in the front subtree to every leaf in the back subtree. To find the actual leaf pairs:

```
distribute_portal(winding, front_child, back_child):
    // Base case: both sides are leaves
    if front_child is Leaf(f) and back_child is Leaf(b):
        if !leaves[f].is_solid and !leaves[b].is_solid:
            emit Portal { polygon: winding, front_leaf: f, back_leaf: b }
        return

    // If front_child is a node, split the winding by that node's plane
    // and recurse: front half goes to the node's front child,
    // back half goes to the node's back child.
    // The other side (back_child) is passed through unchanged.
    if front_child is Node(n):
        (front_winding, back_winding) = split_winding(winding, nodes[n].plane)
        if front_winding is valid:
            distribute_portal(front_winding, nodes[n].front, back_child)
        if back_winding is valid:
            distribute_portal(back_winding, nodes[n].back, back_child)
        return

    // If back_child is a node, same logic but split against back node's plane
    if back_child is Node(n):
        (front_winding, back_winding) = split_winding(winding, nodes[n].plane)
        if front_winding is valid:
            distribute_portal(front_winding, front_child, nodes[n].front)
        if back_winding is valid:
            distribute_portal(back_winding, front_child, nodes[n].back)
        return
```

This produces the correct set of portals — one for each pair of adjacent empty leaves separated by a splitting plane. A single internal node may produce multiple portals if its subtrees contain multiple leaves.

**Degenerate winding rejection:** After every clip or split, reject windings with fewer than 3 vertices or with area below a minimum threshold (e.g., 0.1 square units). This prevents accumulation of slivers from numerical precision loss. This is a key robustness improvement from ericw-tools over the original Quake source.

**Polygon clipping:** Use Sutherland-Hodgman clipping. Extract a shared utility function in a new `src/geometry_utils.rs` (or similar):

```rust
pub fn split_polygon(
    vertices: &[Vec3],
    plane_normal: Vec3,
    plane_distance: f32,
) -> (Option<Vec<Vec3>>, Option<Vec<Vec3>>)
```

Rewrite the existing `split_face` in `bsp.rs` to call `split_polygon` internally, re-attaching the `Face` metadata after the split. Portal generation calls `split_polygon` directly. This keeps the Sutherland-Hodgman intersection math in one place and prevents the two call sites from diverging.

**Epsilon for portal clipping:** The existing `PLANE_EPSILON = 0.1` in `bsp.rs` is appropriate for face classification during BSP building, but is too generous for portal clipping — portals are clipped against many ancestor planes in sequence and accumulate error at each step. Portal clipping should use a tighter epsilon, consistent with ericw-tools' `ON_EPSILON` for winding operations. Define this constant in the portal module:

```rust
const PORTAL_EPSILON: f32 = 0.01;
```

Do not change `PLANE_EPSILON` in `bsp.rs`.

---

## Acceptance Criteria

- `cargo check` and `cargo test -p postretro-level-compiler` pass.
- Unit test: a single box room (6 faces) produces the correct number of portals. A minimal room divided by one splitting plane produces exactly one portal connecting the two halves.
- Unit test: portals generated between solid and empty leaves are not included in the output.
- Unit test: every portal polygon has at least 3 vertices and is planar (all vertices within epsilon of the portal plane).
- Unit test: a two-room map connected by a doorway produces portals at the doorway opening.
- `prl-build` logs portal count after generation. Zero portals is a warning (not an error), since vis will treat all leaves as mutually visible.
- `cargo test -p postretro-level-compiler` — zero failures.
