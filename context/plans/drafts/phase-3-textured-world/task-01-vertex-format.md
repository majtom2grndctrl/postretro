# Task 01: Full Vertex Format and Base Texture UVs

> **Phase:** 3 — Textured World
> **Dependencies:** none. First task in the phase. Requires Phase 1/2 complete.
> **Produces:** vertex buffer with full format (position, base UV, lightmap UV, vertex color), face metadata extended with texture dimensions. Consumed by task-02 (lightmap UV computation) and task-05 (render pipeline).

---

## Goal

Replace the Phase 1 wireframe vertex format (`[f32; 6]`: position + wireframe color) with the full textured vertex format. Compute base texture UVs from BSP texture projection data. Lightmap UVs are placeholder (0,0) — task-02 fills them in. Vertex color is white (1,1,1,1) — Phase 5 uses it for dynamic lighting. Update the vertex buffer layout in the renderer to match the new stride.

---

## Implementation Guidance

### Test map bootstrap

Before any code work, recompile the test map with lighting data:

```
light -bspx assets/maps/test.bsp
```

This writes LIGHTING, RGBLIGHTING, and LIGHTINGDIR lumps into the existing BSP. Verify the file size increased.

Create PNG textures for every texture name referenced by the test map. Place under `textures/<collection>/`. Simple solid-color 64x64 PNGs are sufficient for verification. To find referenced names: load the BSP, iterate `BspMipTexture` entries, print names.

### Vertex format upgrade

Phase 1 uses a `[f32; 6]` vertex (position + wireframe color, 24-byte stride) with `Float32x3` at locations 0 and 1. The wireframe color is computed per-vertex from cluster palette or a constant cyan. Replace this with a struct containing all four textured attributes:

| Field | Type | Default |
|-------|------|---------|
| position | `[f32; 3]` | From BSP vertices (with coordinate transform) |
| base_uv | `[f32; 2]` | Computed from texture projection |
| lightmap_uv | `[f32; 2]` | `[0.0, 0.0]` placeholder (task-02 fills this) |
| vertex_color | `[f32; 4]` | `[1.0, 1.0, 1.0, 1.0]` (white) |

Use `#[repr(C)]` on the struct for GPU layout predictability.

### Base texture UV computation

For each face vertex:

1. Get the face's `BspTexInfo` via `face.texture_info_idx`.
2. Get the texture dimensions from `BspMipTexture.header` via `texinfo.texture_idx`.
3. Project the vertex position (in original Quake Z-up space, before coordinate transform) using `texinfo.projection.project(vertex_position)`. Returns a `Vec2` in texel space.
4. Normalize: divide each component by texture width/height respectively.

The resulting UV is in [0, N] range where N depends on surface size relative to texture size. The GPU sampler wraps via `AddressMode::Repeat`.

### Face metadata extension

Extend the per-face metadata from Phase 1 to include:

- Texture index (into BSP's miptexture array) — needed by task-03 and task-05 for texture binding.
- Texture dimensions (width, height) — needed by task-02 for standard lightmap dimension computation.
- Texture name string — needed by task-03 for PNG matching and task-04 for material derivation.

### Vertex buffer layout

Update the wgpu `VertexBufferLayout` in the renderer (currently `size_of::<[f32; 6]>()` stride at `render.rs:303`) to the new stride (44 bytes) and attribute offsets:

| Location | Offset | Format |
|----------|--------|--------|
| 0 | 0 | Float32x3 (position) |
| 1 | 12 | Float32x2 (base_uv) |
| 2 | 20 | Float32x2 (lightmap_uv) |
| 3 | 28 | Float32x4 (vertex_color) |

### LevelGeometry update

The renderer's `LevelGeometry<'a>` struct currently passes `&'a [[f32; 3]]` vertices. Update to pass the new vertex format data. The `face_cluster_indices` field is wireframe-specific and can be removed (task-05 removes the wireframe pipeline entirely).

### Shader update

Update the vertex shader to accept all four attributes. For this task, the fragment shader can still emit a constant color or use vertex_color as output. Task-05 replaces the shader with texture sampling.

### Coordinate transform interaction

`PlanarTextureProjection::project()` expects Quake-space coordinates. Project UVs before applying the Y-up coordinate transform to position. Alternatively, keep both coordinate systems available during vertex construction and transform position last.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| UV computation coordinate space | Project in Quake Z-up space. Transform position to Y-up afterward. |
| Lightmap UV placeholder | `[0.0, 0.0]`. Task-02 overwrites during atlas construction. |
| Vertex color default | White `[1.0, 1.0, 1.0, 1.0]`. Not consumed until Phase 5. |
| Vertex struct layout | `#[repr(C)]` for GPU alignment. 44-byte stride. |
| Face metadata | Extended with texture index, dimensions, and name. Loader produces, renderer and material system consume. |

---

## Acceptance Criteria

1. Vertex buffer uses the full 44-byte vertex format. All four attributes populated.
2. Base texture UVs are computed for every face vertex. UVs are not all zero or all identical per face (verifiable via debug logging or assertions).
3. Lightmap UVs are placeholder `[0.0, 0.0]` — does not break rendering.
4. Vertex color is white for all vertices.
5. Per-face metadata includes texture index, dimensions, and name for every face.
6. Vertex buffer layout in the renderer matches the new stride and attribute offsets. Shader compiles without errors.
7. Existing wireframe rendering still works with the new vertex format.
8. Test map recompiled with `light -bspx`. PNG textures exist for all referenced texture names.
