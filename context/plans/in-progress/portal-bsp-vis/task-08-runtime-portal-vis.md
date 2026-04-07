# Task 08: Runtime Portal Traversal with Frustum Clipping

**Crates:** `postretro-level-format` · `postretro-level-compiler` · `postretro`
**New files:** `postretro-level-format/src/portals.rs` · `postretro/src/portal_vis.rs`
**Depends on:** Task 07

---

## Context

The precomputed PVS (Tasks 04-05) uses conservative flood-fill: any two empty leaves connected by any chain of portals are marked mutually visible. This correctly culls geometry behind solid brushes but cannot cull geometry around corners — if rooms A, B, C are connected by portals in a chain, A sees C even when no line of sight exists.

Runtime portal traversal with frustum clipping solves this. At render time, the engine walks the portal graph starting from the camera's leaf. At each portal, the current view frustum is clipped against the portal's polygon aperture. If the clipped frustum is empty (the portal is behind the camera, outside the view, or fully occluded by a wall), that branch of the graph is not traversed. This produces exact per-frame visibility that naturally handles corners, narrow doorways, and — in future — dynamic geometry that opens or closes portals.

This task makes runtime portal traversal the **default** visibility mode. The existing precomputed PVS is retained as a compiler option (`--pvs`) for comparison and fallback.

---

## What to Build

### 1. PRL portal section (`postretro-level-format/src/portals.rs`)

New section to store the portal graph in the compiled .prl file.

**Section ID:** `Portals = 15`

**`PortalsSection`** — serializes/deserializes the portal array. Each portal record:
- `vertex_start: u32` — index into a packed vertex array
- `vertex_count: u32` — number of vertices in this portal polygon
- `front_leaf: u32` — leaf index on the front side
- `back_leaf: u32` — leaf index on the back side

The packed vertex array is a flat `Vec<[f32; 3]>` stored contiguously in the section blob, followed by the portal records. This avoids per-portal variable-length encoding.

**Wire format (bytes):**
```
portal_count: u32
vertex_count: u32           (total vertices across all portals)
vertices: [f32; 3] * vertex_count
portals: PortalRecord * portal_count
```

Implement `to_bytes()` and `from_bytes()` with round-trip tests.

Register the section ID in `src/lib.rs`.

### 2. Compiler changes (`postretro-level-compiler`)

**CLI flag:** Add `--pvs` flag to `Args` in `main.rs`. When `--pvs` is passed, the compiler writes sections BspNodes (12), BspLeaves (13), and LeafPvs (14) as today. When `--pvs` is **not** passed (the default), the compiler writes sections BspNodes (12), BspLeaves (13), and Portals (15) — no LeafPvs section.

Both modes always write Geometry (1), BspNodes (12), and BspLeaves (13). The difference is only the visibility data: LeafPvs (14) vs Portals (15).

**`pack.rs`:** Add a variant of `pack_and_write` (or a mode parameter) that accepts `PortalsSection` instead of `LeafPvsSection`. The BspLeaves records in portal mode should have `pvs_offset = 0` and `pvs_size = 0` since no PVS is compiled.

**Portal encoding:** Convert `Vec<Portal>` from `portals.rs` (compiler's internal representation) into `PortalsSection` (format crate's serialized representation). The portal polygons are already in engine coordinates. Leaf indices map directly to the `BspLeavesSection` indices.

### 3. Engine portal loading (`postretro/src/prl.rs`)

**New runtime types:**

```rust
pub struct PortalData {
    pub polygon: Vec<Vec3>,   // convex polygon vertices in world space
    pub front_leaf: usize,
    pub back_leaf: usize,
}
```

**`LevelWorld` additions:**

```rust
pub struct LevelWorld {
    // ... existing fields ...
    pub portals: Vec<PortalData>,    // loaded from Portals section
    pub leaf_portals: Vec<Vec<usize>>, // portal indices per leaf (adjacency)
    pub has_pvs: bool,               // true if LeafPvs section was present
    pub has_portals: bool,           // true if Portals section was present
}
```

When loading a .prl file:
- If section 15 (Portals) is present, load portals and build `leaf_portals` adjacency. Set `has_portals = true`.
- If section 14 (LeafPvs) is present, decompress PVS into leaf records as today. Set `has_pvs = true`.
- Both can be present (engine prefers portals when both are available).
- Neither can be present (fallback to draw-all).

### 4. Runtime portal traversal (`postretro/src/portal_vis.rs`)

**Core algorithm:**

```rust
pub fn portal_traverse(
    camera_position: Vec3,
    camera_leaf: usize,
    frustum: &Frustum,
    world: &LevelWorld,
) -> Vec<bool>
// Returns: visible[leaf_idx] = true if visible this frame
```

**Algorithm — frustum-clipped portal walk:**

1. Initialize `visible: Vec<bool>` of size `leaf_count`, all false. Mark `camera_leaf` visible.
2. Maintain a work queue of `(leaf_index, clipped_frustum)`. Enqueue `(camera_leaf, initial_frustum)`.
3. For each `(current_leaf, current_frustum)` dequeued:
   - For each portal touching `current_leaf` (via `leaf_portals`):
     - Determine the neighbor leaf (the portal's other side).
     - If neighbor is already visible, skip (avoids cycles).
     - If neighbor is solid, skip.
     - Test the portal polygon's AABB against `current_frustum`. If fully outside, skip. (This is the cheap early-out.)
     - Test whether the portal polygon is visible within `current_frustum`: compute the screen-space bounding box of the portal polygon's vertices projected through `current_frustum`, then clip the frustum to the portal's bounding planes. If the result is empty, skip.
     - Mark neighbor visible. Enqueue `(neighbor, narrowed_frustum)`.
4. Return `visible`.

**Frustum narrowing at each portal:** The key operation. Given the current frustum and a convex portal polygon, produce a tighter frustum that only includes sight lines passing through the portal. The simplest correct approach:

For each edge of the portal polygon, construct a plane through the camera position and that edge. This plane clips the frustum from one side. The set of all such planes, combined with the portal's own plane (to clip the near side), forms a new frustum that is the intersection of the old frustum and the pyramid from the camera through the portal. Use only the planes from the portal edges plus the existing far plane from the camera frustum. This replaces the camera's original left/right/top/bottom planes with tighter ones derived from the portal.

If the portal polygon has N vertices, this produces N+1 clipping planes (N edge planes + 1 portal plane as near). Combined with the camera's far plane, this is the new frustum for the neighbor leaf.

The AABB-frustum test already exists (`is_aabb_outside_frustum`). Reuse it with the narrowed frustum for subsequent leaves.

**Important:** The frustum narrows at each portal traversal. After passing through several portals, the frustum may be very tight (a narrow sliver of visibility). This is what provides around-the-corner culling — the frustum shrinks to nothing when sight lines can't pass through the portal chain.

### 5. Integration into visibility pipeline (`postretro/src/visibility.rs`)

**Update `determine_prl_visibility`:**

```rust
pub fn determine_prl_visibility(
    camera_position: Vec3,
    view_proj: Mat4,
    world: &LevelWorld,
) -> (VisibleFaces, VisibilityStats) {
    // ... existing early-outs for empty leaves, BSP descent ...

    if world.has_portals {
        // Runtime portal traversal (new path)
        let visible = portal_traverse(camera_position, camera_leaf_idx, &frustum, world);
        // Collect draw ranges from visible non-solid leaves
        // Apply AABB frustum culling as before
    } else if world.has_pvs {
        // Precomputed PVS path (existing, unchanged)
    } else {
        // Draw-all fallback (existing, unchanged)
    }
}
```

The existing PVS path and draw-all fallback remain unchanged. The portal traversal path is preferred when portal data is present.

---

## Acceptance Criteria

- `cargo check` and `cargo test` pass across the entire workspace.
- Unit test in `postretro-level-format`: `PortalsSection` round-trip (write, read back, verify vertex data and leaf indices).
- Unit test: `prl-build test.map` (default mode) produces a .prl with Portals section (15) and no LeafPvs section (14).
- Unit test: `prl-build test.map --pvs` produces a .prl with LeafPvs section (14) and no Portals section (15).
- Both modes produce BspNodes (12) and BspLeaves (13).
- Unit test: `portal_traverse` on a three-leaf chain (A-B-C) with camera in A looking toward B marks A and B visible but not C (when B's portal to C is outside the frustum).
- Unit test: `portal_traverse` on a straight corridor (A-B-C) with camera in A looking through B into C marks all three visible.
- Unit test: frustum narrowing produces a tighter frustum after passing through a portal.
- `cargo run -p postretro -- assets/maps/test.prl` loads and renders the map with portal-based visibility.
- `cargo test` — zero failures.
