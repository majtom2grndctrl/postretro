# Task 03: Spatial Partitioning

> **Phase:** PRL Phase 1 — Minimum Viable Compiler
> **Dependencies:** Task 02 (compiler scaffold + map parsing).
> **Produces:** Cluster definitions with face assignments, consumed by tasks 04 and 05. BSP tree as compiler-internal intermediate, consumed by task 05 for portal generation.

---

## Goal

Partition world brush geometry into spatial clusters — regions of space with known face membership and bounding volumes. The compiler uses a BSP tree internally to produce these clusters, but the BSP tree is a compiler implementation detail. It is not serialized into the .prl file. The .prl format stores clusters, not BSP nodes.

This task produces two things:
1. **Clusters** — spatial regions with face assignments, bounding volumes, and cluster IDs. These are the format-level concept that the engine sees.
2. **BSP tree** — compiler-internal intermediate used by task 05 to generate portals for PVS computation. Discarded after compilation.

---

## Implementation Guidance

### Input

World brush faces from task 02: polygon vertices in winding order, face planes (normal + distance).

### Coordinate space

BSP construction and clustering operate in shambler's native coordinate space (right-handed, Z-up — Quake convention). Do not transform coordinates in this task. The Y-up coordinate transform is applied later in task 04 (geometry extraction) at serialization time. The BSP tree and cluster definitions produced here use Z-up coordinates throughout.

### Step 1: BSP tree construction

Build a BSP tree from world geometry. This is the standard algorithm:

1. Collect candidate splitting planes from face-aligned planes (each face's plane is a candidate).
2. Score each candidate against the current face set using a cost heuristic.
3. Choose the best-scoring plane. Split the current node.
4. Classify each face as front, back, or spanning. Split spanning faces along the chosen plane.
5. Recurse on the front and back face sets.
6. Terminate when a face set is convex or below a minimum face count threshold.

**Splitting plane heuristic:**
```
score = split_count * split_weight + |front_count - back_count| * balance_weight
```

Lower score is better. Start with `split_weight = 8, balance_weight = 1`. These weights are tunable but not exposed as CLI options in Phase 1.

**Face splitting:** When a face straddles the splitting plane, split it into two polygons using the Sutherland-Hodgman algorithm. Both fragments inherit the original face's plane, texture reference, and metadata. Discard fragments with fewer than 3 vertices.

**Leaf termination:** A node becomes a leaf when all remaining faces are convex, face count is below a threshold (e.g., 4), or no candidate plane produces a meaningful partition.

**Tree storage (compiler-internal):**
- Nodes: splitting plane + front/back child indices + parent index (parent needed for portal generation in task 05).
- Leaves: face index list, axis-aligned bounding box.

### Step 2: Clustering

Group BSP leaves into clusters. Target: 100-300 clusters for a typical level.

Approach: spatial grid-based assignment.

1. Compute the bounding box of all leaves.
2. Divide the bounding box into a regular 3D grid. Grid cell size chosen to produce roughly 100-300 non-empty cells.
3. Assign each leaf to the grid cell containing its centroid.
4. Each non-empty grid cell is a cluster. Assign cluster IDs sequentially.
5. Derive per-cluster data: bounding volume (union of contained leaf bounding boxes), face list (union of contained leaf face lists).

This is intentionally simple. More sophisticated clustering (graph-based, portal-aware) can replace it later without changing the .prl format.

### Output

This task produces:
- **Cluster definitions:** cluster ID, bounding volume, face index list. This is what the engine sees.
- **BSP tree + leaf-to-cluster mapping:** compiler-internal, passed to task 05 for portal generation. Not serialized.

### Validation

After construction, verify:
- Every input face appears in exactly one leaf (accounting for split fragments).
- Every leaf is assigned to exactly one cluster.
- Every face is transitively assigned to exactly one cluster.
- All cluster bounding volumes are finite (no NaN or infinite extents).
- Cluster count is reasonable (for a 5-room map: roughly 10-50 clusters).
- BSP tree depth is reasonable for the input complexity (log-ish, not linear).

---

## Key Decisions

| Item | Resolution |
|------|------------|
| BSP tree in .prl | No. Compiler-internal only. The format stores clusters. |
| Splitting plane candidates | Face-aligned only. No axis-aligned or arbitrary planes. |
| Heuristic weights | split=8, balance=1 as starting point. Tune if test map produces degenerate trees. |
| Face splitting algorithm | Sutherland-Hodgman polygon clipping against the split plane. |
| Leaf termination | Convexity check or face count <= 4 or no useful split. |
| Clustering method | Spatial grid. Simple, deterministic, replaceable later. |
| Target cluster count | 100-300 for a typical level. |

---

## Acceptance Criteria

1. BSP tree builds from the test map's world geometry without panic or infinite recursion.
2. Every input face is accounted for in exactly one cluster (modulo split fragments).
3. Cluster bounding volumes are valid (min < max on all axes, no NaN).
4. Cluster count is within a reasonable range for the test map.
5. Stat log: node count, leaf count, cluster count, total face count (after splits), max tree depth, average faces per cluster.
6. Unit tests verify:
   - A single-brush input produces one cluster containing all faces.
   - A two-room input (two disjoint brush groups) produces at least two clusters.
   - Face splitting produces valid polygons (>= 3 vertices, correct plane classification).
   - Degenerate splits (polygon reduced to < 3 vertices) are discarded.
   - Every face maps to exactly one cluster.
