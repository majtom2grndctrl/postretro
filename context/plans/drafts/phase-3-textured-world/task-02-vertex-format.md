# Task 02: Vertex Format and Base Texture UVs

> **Phase:** 3 — Textured World
> **Dependencies:** none. Requires Phase 1/2 complete.
> **Produces:** vertex buffer with textured format (position, base UV, vertex color), face metadata extended with texture name, index, and dimensions. Consumed by task-04 (material derivation) and task-05 (render pipeline).

---

## Goal

Replace the Phase 1 wireframe vertex format (`[f32; 6]`: position + wireframe color) with the textured vertex format. Compute base texture UVs from BSP texture projection data. No lightmap_uv attribute — Phase 3 uses flat uniform lighting. Vertex color is white (1,1,1,1) — Phase 5 uses it for dynamic lighting. Update the vertex buffer layout in the renderer to match the new stride.

---

## Implementation Guidance

### Test map bootstrap

Before code work, verify PNG textures exist under `textures/<collection>/` for texture names referenced by the test BSP. To find referenced names: load the BSP, iterate `BspMipTexture` entries, print names. Simple solid-color 64x64 PNGs are sufficient for initial verification. The BSP does not need recompilation — Phase 3 uses flat uniform lighting with no baked data.

### Vertex format upgrade

Phase 1 uses a `[f32; 6]` vertex (position + wireframe color, 24-byte stride). Replace with a struct containing the textured attributes:

| Field | Type | Default |
|-------|------|---------|
| position | `[f32; 3]` | From BSP vertices (with coordinate transform) |
| base_uv | `[f32; 2]` | Computed from texture projection |
| vertex_color | `[f32; 4]` | `[1.0, 1.0, 1.0, 1.0]` (white) |

Use `#[repr(C)]` on the struct for GPU layout predictability. Stride: 36 bytes.

No lightmap_uv field. Phase 4 evaluates probe lighting; lightmap UVs are only added if Phase 5 falls back to lightmap atlas.

### Base texture UV computation

For each face vertex:

1. Get the face's `BspTexInfo` via `face.texture_info_idx`.
2. Get the texture dimensions from `BspMipTexture.header` via `texinfo.texture_idx`.
3. Project the vertex position (in original Quake Z-up space, before coordinate transform) using `texinfo.projection.project(vertex_position)`. Returns a `Vec2` in texel space.
4. Normalize: divide by texture width and height respectively.

The resulting UV is in [0, N] range where N depends on surface size relative to texture size. The GPU sampler wraps via `AddressMode::Repeat`.

### Coordinate transform interaction

`PlanarTextureProjection::project()` expects Quake-space coordinates. Project UVs before applying the Y-up coordinate transform to vertex position. Alternatively, maintain both coordinate systems during vertex construction and transform position last.

### Face metadata extension

Extend the per-face metadata from Phase 1 to include:

- Texture index (into BSP's miptexture array) — needed by task-02 and task-04 for texture binding.
- Texture dimensions (width, height) — available for future phases; stored here while parsing.
- Texture name string — needed by task-02 for PNG matching and task-03 for material derivation.

### Vertex buffer layout

Update the wgpu `VertexBufferLayout` in the renderer to the new stride (36 bytes) and attribute offsets:

| Location | Offset | Format |
|----------|--------|--------|
| 0 | 0 | Float32x3 (position) |
| 1 | 12 | Float32x2 (base_uv) |
| 2 | 20 | Float32x4 (vertex_color) |

### LevelGeometry update

The renderer's `LevelGeometry<'a>` struct currently passes `&'a [[f32; 3]]` vertices. Update to pass the new vertex format data. The `face_cluster_indices` field is wireframe-specific — remove it (task-04 removes the wireframe pipeline entirely).

### Shader stub

Update the vertex shader to accept the three attributes. For this task, the fragment shader can emit vertex_color as output or a constant color. Task-04 replaces the shader with full texture sampling and uniform lighting.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| UV computation coordinate space | Project in Quake Z-up space. Transform position to Y-up afterward. |
| No lightmap_uv | Omitted. Phase 3 uses flat uniform lighting. Phase 4+ decides whether lightmap UVs are needed. |
| Vertex color default | White `[1.0, 1.0, 1.0, 1.0]`. Not consumed until Phase 5. |
| Vertex struct layout | `#[repr(C)]` for GPU alignment. 36-byte stride. |
| Face metadata | Extended with texture index, dimensions, and name. Loader produces, renderer and material system consume. |

---

## Acceptance Criteria

1. Vertex buffer uses the 36-byte vertex format. All three attributes populated.
2. Base texture UVs computed for every face vertex. UVs are not all zero or all identical per face.
3. Vertex color is white for all vertices.
4. Per-face metadata includes texture index, dimensions, and name for every face.
5. Vertex buffer layout in the renderer matches the new stride and attribute offsets. Shader compiles without errors.
6. Existing wireframe rendering still works with the new vertex format (or a debug flag toggles it).
