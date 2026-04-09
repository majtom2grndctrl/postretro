# Task 01: CSG Face Clipping

> **Phase:** 3 — Textured World
> **Dependencies:** none. Runs in the PRL compiler, independent of engine rendering tasks.
> **Produces:** PRL geometry with overlapping-brush z-fighting eliminated. Faces inside solid brush space are removed or clipped at compile time.

---

## Goal

Clip PRL faces against overlapping brush volumes during compilation. Removes faces that lie inside solid space — the source of z-fighting when two coplanar faces from adjacent brushes occupy the same depth. This is a compile-time step in `postretro-level-compiler`. No engine changes.

See `context/reference/csg-face-clipping.md` for background and algorithm detail.

---

## Implementation Guidance

### Why PRL-only

BSP already handles this via BSP tree construction — splitting geometry along brush planes eliminates overlaps structurally. PRL geometry is extracted directly from brush faces without BSP splitting, so overlapping brushes produce duplicate geometry at shared surfaces.

### Existing clipping utilities

The compiler already has Sutherland-Hodgman clipping in `geometry_utils.rs`:

- `split_polygon(vertices, normal, distance, epsilon)` → `(Option<front>, Option<back>)` — splits a convex polygon by a plane
- `clip_polygon_to_front(vertices, normal, distance, epsilon)` → `Option<Vec<Vec3>>` — keeps the front side, discards back

These are currently used by BSP partitioning and portal generation. This task wires them into a new CSG elimination pass — no new clipping algorithm needed.

### Algorithm

For each face in the compiled geometry:

1. **AABB pre-filter.** Test the face's bounding box against every other brush. Skip brushes whose AABB doesn't intersect the face AABB.
2. **Clip against brush half-planes.** For each intersecting brush, use `clip_polygon_to_front()` against each of the brush's half-planes (negated, to clip away the inside-solid side). If the polygon survives all planes, it's outside that brush.
3. **Discard test.** If clipping reduces the polygon to nothing (fully inside a brush), discard the face entirely.
4. **Partial clip.** If partially inside, keep the remaining polygon. Re-triangulate if needed.

Clip against all brush volumes, including the face's own brush. A face on its own brush's boundary plane is not "inside" that brush (it sits on the plane, not behind all half-planes), so the face's own brush will not clip it. No `brush_index` tracking is needed. The epsilon for the inside test must be strictly negative — a point exactly on a plane is not behind it.

### Input data

The compiler already has:
- Brush volumes with half-planes (parsed from `.map` via shambler)
- Face polygons (vertices extracted per-brush-face)
- Clipping utilities in `geometry_utils.rs`

### Voxel grid alternative

The existing voxel grid provides a coarser option: test face vertices and centroid against solid voxels. Discard faces where all test points land in solid voxels. Less precise than geometric clipping (misses partial-overlap cases) but trivial to implement with existing infrastructure. Consider as a first pass if geometric clipping is complex.

### Output

Clipped geometry feeds directly into the existing PRL pack step. The compiler's geometry representation should support polygon modification in-place or produce a new list of clipped polygons.

### Verification

Compile a `.map` with two overlapping room brushes sharing a wall plane. Without clipping: z-fighting visible at the shared wall in textured rendering. With clipping: only one face exists at the shared plane, no z-fighting.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| Compiler vs engine | Compile-time only. No engine changes. PRL already stores the clipped result. |
| Algorithm | Sutherland-Hodgman geometric clip via existing `geometry_utils.rs`. Voxel discard as fallback if geometric clip is impractical. |
| AABB pre-filter | Required. Naive O(faces × brushes) without it is too slow for large maps. |
| Brush generating the face | Not excluded. Clip against all brushes including own. A face on its own brush boundary is not geometrically "inside" that brush (on the plane, not behind all half-planes), so no self-clipping occurs. No `brush_index` field needed. Epsilon must be strictly negative for inside test. |
| Re-triangulation | Required when partial clip produces a non-triangle polygon. Fan triangulation from centroid is sufficient. |

---

## Acceptance Criteria

1. Compiling a `.map` with overlapping brushes produces a PRL with no duplicate coplanar faces at shared brush boundaries.
2. Textured PRL rendering (when PRL texture support is added) shows no z-fighting at brush seams.
3. Fully-inside faces (e.g., a brush face entirely inside a solid neighbor) are discarded — not written to the PRL.
4. Partially-inside faces are clipped to only the outside portion.
5. Compile time remains acceptable for maps with O(100) brushes. AABB pre-filter confirmed active.
6. Non-overlapping brushes produce identical output to the unclipped path (clipping is a no-op for separated geometry).
