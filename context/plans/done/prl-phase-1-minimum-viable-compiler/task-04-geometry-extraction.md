# Task 04: Geometry Extraction

> **Phase:** PRL Phase 1 — Minimum Viable Compiler
> **Dependencies:** Task 03 (spatial partitioning — provides cluster definitions with face assignments).
> **Produces:** Geometry section data (vertices, indices, face metadata organized by cluster) consumed by task 06.

---

## Goal

Extract renderable geometry from the cluster definitions produced by task 03. Fan-triangulate face polygons into vertex and index buffers. Organize faces by cluster so the engine can draw all faces for a given cluster as a contiguous range. Transform coordinates from Quake convention to engine convention. Produce data ready for serialization into the Geometry section of the .prl file.

---

## Implementation Guidance

### Fan triangulation

Each face polygon is a convex polygon. Triangulate using fan decomposition: for vertices [v0, v1, ..., vN], emit triangles (v0, v1, v2), (v0, v2, v3), ..., (v0, v(N-1), vN).

Build:
- A flat vertex buffer: `Vec<[f32; 3]>` of positions.
- A flat index buffer: `Vec<u32>` of triangle indices.
- Per-face metadata: index buffer offset, index count, cluster index.

### Coordinate transform

Quake uses right-handed Z-up (+X forward, +Y left, +Z up). The engine uses right-handed Y-up (+X right, +Y up, -Z forward). The compiler performs this transform at extraction time so the .prl stores engine-native coordinates. The engine does no coordinate conversion at load time.

Apply the transform to all vertex positions and to cluster bounding volumes before serialization.

### Face ordering by cluster

Faces are ordered by cluster — all faces in cluster 0, then cluster 1, etc. This enables contiguous draw ranges: each cluster has a start index and count into the face metadata array. The engine draws an entire cluster's worth of faces with one draw call per cluster.

Per-cluster face ranges (start index into face metadata array, face count) are stored in the Cluster Visibility section (task 05), not in the Geometry section. This task ensures faces are ordered by cluster so that contiguous ranges exist; task 05 records the actual range boundaries.

### Vertex deduplication

Optional: deduplicate vertices that share the same position (within a small epsilon). Skip in Phase 1 unless the vertex count is problematically large for the test map.

### Section data type

Define a Geometry section data type in postretro-level-format:
- Vertex count, index count, face count, cluster count
- Vertex position array
- Index array
- Per-face metadata array (index offset, index count, cluster index)
Implement serialization (for the compiler) and deserialization (for the engine loader in task 07).

Note: per-cluster face ranges are owned by the Cluster Visibility section (task 05), not the Geometry section. The Geometry section stores face metadata with cluster indices, enabling the Cluster Visibility section to reference contiguous face ranges.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| Coordinate convention in .prl | Engine Y-up. Compiler transforms at extraction time. |
| Vertex format | Position-only `[f32; 3]` for Phase 1. |
| Index format | u32 |
| Vertex deduplication | Skip in Phase 1 unless vertex count is problematic. |
| Face ordering | Faces ordered by cluster. All faces for a given cluster occupy a contiguous range. |

---

## Acceptance Criteria

1. Every face produces at least one triangle (3+ vertices produces 1+ triangles).
2. Index buffer contains no out-of-bounds references.
3. Per-face metadata accounts for every triangle in the index buffer (sum of all face index counts == total index count).
4. Coordinate transform is applied: vertex Y values correspond to vertical, Z values correspond to depth. Verify against known test map geometry (a floor brush should have vertices with similar Y values).
5. Faces are ordered by cluster. All faces for a given cluster occupy a contiguous range in the face metadata array.
6. Per-cluster face ranges are correct: start + count covers exactly the faces assigned to that cluster.
7. Round-trip test: serialize geometry section, deserialize, compare — all values match.
8. Unit tests verify fan-triangulation produces correct index patterns for 3, 4, 5, and 6-vertex polygons.
