# Task 05: Textured Render Pipeline

> **Phase:** 3 — Textured World
> **Dependencies:** task-01 (full vertex format), task-02 (lightmap atlas), task-03 (loaded textures).
> **Produces:** solid textured rendering of BSP geometry. Replaces the Phase 1 wireframe pipeline.

---

## Goal

Replace the wireframe render pipeline with a solid textured pipeline. Add a depth buffer and back-face culling. Shader samples base texture and lightmap atlas per fragment. Draw calls are grouped by texture to minimize bind group changes.

---

## Implementation Guidance

### Depth buffer

Create a depth texture matching the surface dimensions:

| Parameter | Value |
|-----------|-------|
| Format | `TextureFormat::Depth24Plus` (prefer) or `Depth32Float` if unavailable |
| Usage | `RENDER_ATTACHMENT` |
| Size | Match surface width/height |
| Recreate on | Window resize (same as surface reconfiguration) |

Attach to the render pass as `depth_stencil_attachment`:
- `depth_load_op: LoadOp::Clear(1.0)`
- `depth_store_op: StoreOp::Store`
- `depth_compare: CompareFunction::Less`
- Stencil: default (not used)

Enable depth write and depth test in the pipeline's `DepthStencilState`.

### Back-face culling

Set `primitive.cull_mode = Some(Face::Back)` in the pipeline descriptor.

Front face winding: `FrontFace::Ccw` (counter-clockwise). Verify this matches the face winding after Phase 1's coordinate transform. If faces disappear (culled wrong side), try `FrontFace::Cw`. The correct winding depends on whether the coordinate transform (Y/Z swap + Z negate) reverses winding order.

### GPU texture upload

For each loaded texture from task-03:
1. Create a wgpu `Texture` (`Rgba8UnormSrgb` format, dimensions from loaded data).
2. Write pixel data via `queue.write_texture()`.
3. Create a `TextureView`.

For the lightmap atlas from task-02:
1. Create a wgpu `Texture` (`Rgba8Unorm` format — lightmaps are linear data, not sRGB).
2. Write atlas pixel data via `queue.write_texture()`.
3. Create a `TextureView`.

Create two `Sampler`s:
- **Base texture sampler:** `FilterMode::Nearest` mag/min (retro pixel aesthetic), `AddressMode::Repeat` (textures tile).
- **Lightmap sampler:** `FilterMode::Linear` mag/min (smooth lightmap interpolation), `AddressMode::ClampToEdge`.

### Bind group layout

Three bind groups:

| Group | Binding | Resource | Frequency |
|-------|---------|----------|-----------|
| 0 | 0 | Uniform buffer (view-projection matrix) | Per frame |
| 1 | 0 | Lightmap atlas texture view | Per level |
| 1 | 1 | Lightmap sampler | Per level |
| 2 | 0 | Base texture view | Per texture |
| 2 | 1 | Base texture sampler | Per texture |

Group 0 changes per frame. Group 1 is bound once at level load. Group 2 changes per texture batch.

### Shader

**Vertex shader:**
- Inputs: position (vec3), base_uv (vec2), lightmap_uv (vec2), vertex_color (vec4)
- Uniform: view-projection matrix (mat4x4)
- Transform position by view-projection. Pass UVs and vertex_color to fragment shader.

**Fragment shader:**
- Sample base texture at `base_uv`.
- Sample lightmap atlas at `lightmap_uv`.
- Output: `base_color.rgb * lightmap_color.rgb * vertex_color.rgb`, alpha = `base_color.a * vertex_color.a`.

Lightmap modulation is multiplicative: (0.5, 0.5, 0.5) halves base brightness. (1.0, 1.0, 1.0) is fully lit.

**sRGB handling:** Base textures stored as Rgba8UnormSrgb — wgpu hardware converts to linear on sample. Lightmap atlas stored as Rgba8Unorm (already linear). Multiplication happens in linear space. sRGB surface format handles final linear-to-sRGB conversion. No manual gamma correction needed.

### Draw call structure

Group faces by texture index. For each texture group:
1. Set bind group 2 to the texture's bind group.
2. Issue one `draw_indexed()` covering the contiguous index range.

Optimization: sort the index buffer by texture index at load time so faces sharing a texture are contiguous. This enables one draw call per texture group instead of one per face.

Integrate with PVS and frustum culling: only draw faces in the visible set. Group visible faces by texture, then draw.

### Removing wireframe

Phase 1's wireframe renderer has significant infrastructure to remove or gate behind a debug flag:

- `WireframeMode` enum (`PolygonModeLine` vs `LineList` fallback)
- `build_line_list_indices()` — reconstructs face edges for LineList topology
- `build_colored_vertices()` — assigns cluster palette or default cyan to each vertex
- `CLUSTER_PALETTE` and `DEFAULT_WIREFRAME_COLOR` constants
- LineList-specific draw path branch in `render_frame()`
- `LevelGeometry::face_cluster_indices` field (wireframe-only)
- `--force-line-list` CLI flag handling

Default path becomes solid textured. If wireframe is useful for debugging, keep it behind a debug flag — but do not maintain two parallel vertex buffer construction paths.

### Pipeline descriptor

| Field | Value |
|-------|-------|
| `primitive.topology` | `TriangleList` |
| `primitive.front_face` | `Ccw` (verify with coordinate transform) |
| `primitive.cull_mode` | `Some(Back)` |
| `depth_stencil` | Depth24Plus, compare Less, write true |
| `multisample` | count 1 (no MSAA) |
| `fragment.targets` | Surface format (sRGB) |

---

## Key Decisions

| Item | Resolution |
|------|------------|
| Depth format | Depth24Plus preferred. Depth32Float fallback. |
| Base texture filtering | Nearest (retro pixel aesthetic). |
| Lightmap filtering | Linear (smooth lighting interpolation). |
| Base texture format | Rgba8UnormSrgb (hardware sRGB decode). |
| Lightmap atlas format | Rgba8Unorm (linear data). |
| Bind group structure | 3 groups: per-frame uniforms, per-level lightmap, per-texture base. |
| Draw call grouping | Sort index buffer by texture at load time. One draw call per texture group in visible set. |
| Winding order | Ccw front face. Verify empirically — flip if back-face culling hides interior faces. |

---

## Acceptance Criteria

1. BSP geometry renders as solid textured surfaces. No wireframe, no untextured faces.
2. Base textures correctly mapped. Orientation and scale match BSP geometry — no stretching, no rotation errors.
3. Lightmap modulation visible: shadowed areas darker, lit areas brighter. No faces uniformly black or white unless lighting data dictates it.
4. Depth buffer eliminates z-fighting. Near geometry occludes far geometry.
5. Back-face culling reduces face count. Interior walls visible; exterior back-faces culled.
6. Checkerboard placeholder textures render correctly for missing PNGs.
7. Colored lightmaps (RGBLIGHTING) tint surfaces when present. Monochrome fallback works without visual artifacts.
8. Draw calls grouped by texture. Diagnostic logging shows draw call count significantly less than visible face count.
9. PVS and frustum culling still function. Draw counts change when navigating between rooms.
10. Window resize recreates depth buffer without artifacts.
