# Task 01: BSP Loading and Geometry Extraction

> **Phase:** 1 — BSP Loading and Wireframe
> **Dependencies:** none. First task in the phase.
> **Produces:** owned vertex/index data and per-face metadata consumed by task-02.

---

## Goal

Parse a BSP2 file with the qbsp crate. Extract geometry and build engine-side vertex and index buffers ready for GPU upload. Establish the BSP loader module boundary: this code parses BSP data and produces engine types. It never touches wgpu.

---

## Implementation Guidance

### BSP parsing

Use qbsp 0.14 to open and parse the BSP2 file. Extract:

- Vertices (position data)
- Edges and face-edge lists (surf edges)
- Faces (polygon definitions via edge loops)
- Models (model 0 is the world geometry)
- Visibility data (compressed PVS bitfield — store raw, consumed by task-04)
- Leaf data (leaf bounding boxes, leaf-to-face mappings)
- Node/plane data (BSP tree structure for point-in-leaf queries, consumed by task-04)

### Vertex and index buffer construction

BSP faces are convex polygons defined by edge loops. Fan-triangulate each face:

- For a face with vertices `[v0, v1, v2, ..., vN]`, emit triangles: `(v0, v1, v2)`, `(v0, v2, v3)`, ..., `(v0, v(N-1), vN)`.
- Vertex format: position only — `[f32; 3]`. Full vertex format (UV, lightmap UV, vertex color) arrives in Phase 3.
- Index format: `u32`.

Produce:
- A flat `Vec<[f32; 3]>` of vertex positions.
- A flat `Vec<u32>` of triangle indices.
- Per-face metadata: index offset into the index buffer, index count, which leaf the face belongs to.

### Coordinate transform

Quake BSP uses right-handed Z-up. The engine uses Y-up (glam default). During vertex loading, apply: swap Y and Z, negate the new Z. Verify visually in task-02 that geometry is correct (walls are walls, floors are floors).

### BSP file path

Accept an optional CLI argument for the BSP path. If absent, fall back to `assets/maps/test.bsp`. Fail with a clear error message if no BSP file is found.

### Module boundary

The loader is a separate module from the renderer. It returns owned engine types. No wgpu imports in loader code.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| Coordinate handedness | Swap Y/Z, negate new Z during load. Verify visually in task-02. |
| Face winding | May flip after coordinate transform. Wireframe doesn't cull backfaces, but log/inspect normals now so Phase 3 doesn't inherit a hidden bug. |
| Vertex format | Position-only `[f32; 3]` for Phase 1. Buffer layout changes in Phase 3 — don't over-abstract. |
| BSP file path | CLI arg with `assets/maps/test.bsp` fallback. |

---

## Test Map Bootstrap

This task needs a BSP file. No textures, no lighting — just geometry and visibility data.

### Setup

1. Install TrenchBroom and ericw-tools 2.0.0-alpha.
2. Use the default Quake game config in TrenchBroom (custom FGD and entities come in later phases).
3. Author a test map: 3-5 rooms connected by corridors with vertical variation (stairs, a ledge, a multi-story atrium). Enough spatial complexity to verify PVS culling in task-04.
4. Compile:

```
qbsp -bsp2 -notex -wrbrushes test.map
vis test.bsp
```

No `light` step — wireframe doesn't use lightmaps. `-wrbrushes` costs nothing and keeps the BSP ready for later phases.

5. Place the compiled `test.bsp` in `assets/maps/`.

### Map design tips for PVS verification

- At least one L-shaped corridor so rooms on either side are not mutually visible.
- One large open area for high face counts.
- One small enclosed room for minimal draw sets.

The test map persists across all phases as a development fixture.

---

## Acceptance Criteria

1. `cargo run -- assets/maps/test.bsp` parses the BSP without error.
2. Loader produces vertex and index data with correct counts (no empty buffers, no out-of-bounds indices).
3. Per-face metadata (index offset, count, leaf membership) is populated for every face.
4. Visibility data and BSP tree structure are stored for consumption by task-04.
5. Loader module contains zero wgpu imports.
6. Missing BSP file produces a clear error message, not a panic with a cryptic path.
