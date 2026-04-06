# Task 05: Portal Generation and PVS Computation

> **Phase:** PRL Phase 1 — Minimum Viable Compiler
> **Dependencies:** Task 03 (spatial partitioning — provides BSP tree and cluster definitions).
> **Produces:** Cluster Visibility section data (cluster bounding volumes, PVS bitsets) consumed by task 06.

---

## Goal

Generate portals from the compiler-internal BSP tree, reduce them to inter-cluster boundaries, and compute a cluster-to-cluster potentially visible set (PVS). Store the result as compressed bitsets alongside cluster bounding volumes. This is the precomputed visibility data that lets the engine skip 70-90% of invisible geometry per frame.

---

## Implementation Guidance

### Coordinate space

Portal generation and PVS computation operate in shambler's native coordinate space (right-handed, Z-up — Quake convention), matching the BSP tree produced by task 03. Cluster bounding volumes must be transformed to engine Y-up coordinates before serialization into the Cluster Visibility section. Apply the same Z-up → Y-up transform used in task 04 (geometry extraction).

### Test map fixtures

PVS correctness depends on level geometry. The test .map file(s) must include:
- **Multi-room layout:** at least 3 rooms connected by corridors or doorways, so that some cluster pairs have visibility and others don't.
- **Occluding wall variant:** a version with a solid wall blocking the sightline between two rooms. Used to verify that PVS correctly reports non-visibility between occluded clusters.

If `assets/maps/test.map` doesn't already provide these configurations, create additional small .map files (e.g., `assets/maps/test_pvs_occluded.map`) for unit test use.

### Portal generation

Portals are polygons that represent the open boundaries between adjacent BSP leaves — the "windows" through which visibility can pass. Generating them from the BSP tree is a well-established algorithm (Quake's QBSP uses this exact approach).

**Data structures:**

Each portal stores:
- A winding (convex polygon vertices)
- Two leaf indices: front and back
- Each leaf maintains a list of its portals

**Algorithm (two phases):**

**Phase A — bounding box portals.** Compute the axis-aligned bounding box of all geometry, padded by a margin. Create 6 portals (one per box face) connecting the root node to a sentinel "outside" leaf marked as solid. These define the initial closed volume.

**Phase B — recursive portal splitting.** Traverse the BSP tree in pre-order. At each internal node:

1. **Create a new portal on this node's splitting plane.**
   - Start with a large polygon lying on the splitting plane (a quad extending well beyond the level bounds).
   - Walk up from this node to the root, clipping the polygon against each ancestor's splitting plane. At each ancestor, clip to the front or back half-space depending on which child leads to the current node.
   - The surviving polygon is the portal between this node's front and back children.
   - If the polygon is degenerate after clipping (null or below a minimum area/edge-length threshold), discard it.

2. **Split all existing portals at this node.**
   - For each portal currently in this node's portal list, clip its winding against the splitting plane using `DivideWinding` (Sutherland-Hodgman producing both front and back fragments).
   - Front fragment: re-add connecting front child to the portal's other leaf.
   - Back fragment: re-add connecting back child to the portal's other leaf.
   - If only one fragment survives, the portal moves entirely to that child.
   - Discard tiny/degenerate fragments.

3. **Recurse** on front and back children. Stop at leaves.

After traversal completes, each leaf has a list of portals connecting it to adjacent leaves.

**Filtering:** Discard portals where either adjacent leaf is solid (outside the playable volume). Determine solid vs. empty by flood-filling from entity positions (e.g., info_player_start) through portals. Leaves not reached by the flood are solid.

**Polygon clipping primitive.** The fundamental operation (`ClipWinding`): clip a convex polygon against a plane, producing a (possibly smaller) convex polygon on the front side. Use an epsilon (0.1 units) for point-on-plane classification to avoid numerical instability. `DivideWinding` is the same operation but returns both front and back fragments.

### Cluster-level portal reduction

Using the leaf-to-cluster mapping from task 03:

1. Discard portals between two leaves in the same cluster (intra-cluster visibility is assumed).
2. Retain portals between leaves in different clusters. These are the inter-cluster portals used for PVS.

### PVS computation

For each cluster, determine which other clusters are potentially visible through portal chains.

**Algorithm: BasePortalVis (pairwise plane test + flood fill).** This is the conservative first pass from Quake's vis tool — fast, correct, and sufficient for Phase 1.

**Step 1 — pairwise plane test.** For each portal `p`, test every other portal `q`:
- If no vertex of `q`'s winding is in front of `p`'s plane, `q` is behind `p` and invisible. Skip.
- If no vertex of `p`'s winding is behind `q`'s plane, `p` faces away from `q`. Skip.
- If both tests pass, mark `q` as a candidate visible from `p`.

This eliminates geometrically impossible visibility pairs cheaply (dot products only, no clipping).

**Step 2 — flood fill.** For each portal `p`, flood-fill through candidate-visible portals:
- Mark the current cluster as visible in `p`'s bitset.
- For each portal in this cluster that passed the pairwise test, recurse into the portal's destination cluster.
- The flood propagates only through portals that passed the geometric test.

**Result:** Each cluster has a `mightsee` bitset. This is conservative — it may include clusters that aren't actually visible, but never excludes truly visible ones. The engine's frustum culling handles the over-estimation at runtime.

**Why not depth-limited flood:** The pairwise plane test is barely more complex than a depth check but produces much better results because it respects geometry rather than using an arbitrary distance cutoff.

**Future tightening:** The full anti-penumbra algorithm (Quake's `RecursiveLeafFlow`) can be added later as an optimization pass. It tracks the visible cone through portal chains using separating planes, typically reducing the visible set by 30-50%. The data structures from Phase 1 support this directly.

### PVS storage

For each cluster, store a bitset where bit N indicates whether cluster N is visible.

Compress using run-length encoding (Quake-style): sequences of zero bytes are stored as `0x00, count`. Non-zero bytes are stored verbatim. Simple, fast to decompress, and effective for the typical PVS pattern (mostly zeros with scattered ones).

### Section data type

Define a Cluster Visibility section data type in postretro-level-format:
- Cluster count
- Per-cluster data: bounding volume (AABB), face range (start index + count, referencing the Geometry section's face metadata array)
- Per-cluster PVS: offset + size into a shared compressed PVS data blob
- Compressed PVS blob

The cluster bounding volumes serve double duty: frustum culling (skip clusters outside the view) and point-in-cluster queries (determine which cluster the camera is in). For 100-300 clusters, a linear scan of bounding volumes is sufficient — no spatial index needed.

Implement serialization and deserialization.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| Portal generation | BSP tree traversal with ancestor clipping (Quake QBSP algorithm). Operates on compiler-internal BSP tree, not serialized. |
| Portal filtering | Flood-fill from entity positions to identify solid leaves. Discard portals bordering solid. |
| PVS algorithm | BasePortalVis: pairwise portal plane tests + flood fill. Conservative (over-inclusive). |
| Anti-penumbra | Deferred to future phase. Phase 1 PVS is correct but loose. |
| PVS compression | Run-length encoding (0x00, count for zero runs). |
| PVS correctness guarantee | Never under-estimates visibility. May over-estimate. |
| Point-in-cluster at runtime | Linear scan of cluster bounding volumes. Sufficient for 100-300 clusters. |
| Clipping epsilon | 0.1 units for point-on-plane classification. |

---

## Acceptance Criteria

1. Portal generation produces portals between BSP leaves without panic.
2. Cluster-level portal reduction correctly discards intra-cluster portals and retains inter-cluster portals.
3. PVS is symmetric: if cluster A can see cluster B, then B can see A.
4. PVS is reflexive: every cluster can see itself.
5. PVS is conservative: no false negatives. A cluster that is geometrically visible from another cluster is marked visible. (Verify with known sightlines in the test map.)
6. Compressed PVS round-trips correctly: compress, decompress, compare to original bitset.
7. Stat log: portal count (before/after filtering), cluster count, PVS density (percentage of cluster pairs marked visible), compressed PVS size.
8. Unit tests verify:
   - RLE compression/decompression round-trips for known bitset patterns (all zeros, all ones, sparse, dense).
   - A two-cluster level with a clear sightline has mutual visibility.
   - A two-cluster level with an occluding wall between them does not have mutual visibility.
