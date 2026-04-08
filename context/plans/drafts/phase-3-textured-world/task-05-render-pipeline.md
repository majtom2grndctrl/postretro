# Task 05: Render Pipeline

> **Phase:** 3 — Textured World
> **Dependencies:** task-02 (full vertex format), task-03 (loaded textures).
> **Produces:** solid textured rendering of BSP geometry with flat uniform lighting. Replaces the Phase 1 wireframe pipeline.

---

## Goal

Replace the wireframe render pipeline with a solid single-texture pipeline. Add a depth buffer and back-face culling. Shader samples base texture and multiplies by a flat ambient uniform. Draw calls are grouped by texture to minimize bind group changes. Phase 4 replaces the flat uniform with probe-sampled lighting.

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

Front face winding: `FrontFace::Ccw` (counter-clockwise). Verify this matches face winding after Phase 1's coordinate transform. If faces disappear (culled wrong side), try `FrontFace::Cw`. The correct winding depends on whether the coordinate transform (Y/Z swap + Z negate) reverses winding order.

### GPU texture upload

For each loaded texture from task-03:
1. Create a wgpu `Texture` (`Rgba8UnormSrgb` format, dimensions from loaded data).
2. Write pixel data via `queue.write_texture()`.
3. Create a `TextureView`.

Create a base texture `Sampler`:
- `FilterMode::Nearest` mag/min (retro pixel aesthetic)
- `AddressMode::Repeat` (textures tile)

### Bind group layout

Two bind groups:

| Group | Binding | Resource | Frequency |
|-------|---------|----------|-----------|
| 0 | 0 | Uniform buffer (view-projection matrix + ambient lighting factor) | Per frame |
| 1 | 0 | Base texture view | Per texture |
| 1 | 1 | Base texture sampler | Per texture |

Group 0 changes per frame. Group 1 changes per texture batch.

The uniform buffer in group 0 holds the view-projection matrix and a flat ambient `vec3` (e.g., `[1.0, 1.0, 1.0]`). Phase 4 replaces this with probe-sampled values.

### Shader

**Vertex shader:**
- Inputs: position (vec3), base_uv (vec2), vertex_color (vec4)
- Uniform: view-projection matrix (mat4x4), ambient_light (vec3)
- Transform position by view-projection. Pass base_uv and vertex_color to fragment shader.

**Fragment shader:**
- Sample base texture at `base_uv`.
- Output: `base_color.rgb * ambient_light * vertex_color.rgb`, alpha = `base_color.a * vertex_color.a`.

Flat lighting is multiplicative: `ambient_light = [1.0, 1.0, 1.0]` is fully lit. Phase 4 replaces `ambient_light` with a per-surface probe-sampled value.

**sRGB handling:** Base textures stored as `Rgba8UnormSrgb` — wgpu hardware converts to linear on sample. sRGB surface format handles final linear-to-sRGB conversion. No manual gamma correction needed.

### Draw call structure

Group faces by texture index. For each texture group:
1. Set bind group 1 to the texture's bind group.
2. Issue one `draw_indexed()` covering the contiguous index range.

Sort the index buffer by texture index at load time so faces sharing a texture are contiguous. One draw call per texture group in the visible set.

Integrate with PVS and frustum culling: only draw faces in the visible set. Group visible faces by texture, then draw.

### Removing wireframe

Phase 1's wireframe renderer has infrastructure to remove or gate behind a debug flag:

- `WireframeMode` enum (`PolygonModeLine` vs `LineList` fallback)
- `build_line_list_indices()` — reconstructs face edges for LineList topology
- `build_colored_vertices()` — assigns cluster palette or default cyan to each vertex
- `CLUSTER_PALETTE` and `DEFAULT_WIREFRAME_COLOR` constants
- LineList-specific draw path branch in `render_frame()`
- `LevelGeometry::face_cluster_indices` field (wireframe-only)
- `--force-line-list` CLI flag handling

Default path becomes solid textured. If wireframe is useful for debugging, keep it behind a debug flag — do not maintain two parallel vertex buffer construction paths.

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
| Lighting model | Flat uniform ambient. Single `vec3` in the per-frame uniform buffer. Phase 4 replaces with probe sampling. |
| No lightmap atlas | Removed from Phase 3. No second texture, no lightmap bind group. |
| Depth format | Depth24Plus preferred. Depth32Float fallback. |
| Base texture filtering | Nearest (retro pixel aesthetic). |
| Base texture format | Rgba8UnormSrgb (hardware sRGB decode). |
| Bind group structure | 2 groups: per-frame uniforms (including ambient), per-texture base texture. |
| Draw call grouping | Sort index buffer by texture at load time. One draw call per texture group in visible set. |
| Winding order | Ccw front face. Verify empirically — flip if back-face culling hides interior faces. |

---

## Acceptance Criteria

1. BSP geometry renders as solid textured surfaces. No wireframe, no untextured faces.
2. Base textures correctly mapped. Orientation and scale match BSP geometry — no stretching, no rotation errors.
3. Uniform flat lighting applied. All surfaces at the same brightness. No shadow variation. (Phase 4 adds probe lighting.)
4. Depth buffer eliminates z-fighting. Near geometry occludes far geometry.
5. Back-face culling reduces face count. Interior walls visible; exterior back-faces culled.
6. Checkerboard placeholder textures render correctly for missing PNGs.
7. Draw calls grouped by texture. Diagnostic logging shows draw call count significantly less than visible face count.
8. PVS and frustum culling still function. Draw counts change when navigating between rooms.
9. Window resize recreates depth buffer without artifacts.
10. Module boundary holds: texture loader contains zero wgpu imports; renderer does not parse BSP or PNG data.
