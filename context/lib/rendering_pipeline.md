# Rendering Pipeline

> **Read this when:** implementing or modifying the renderer, level loading, lighting, or any visual pass.
> **Key invariant:** renderer owns all wgpu calls. Other subsystems never touch GPU types. Level loaders produce handles; renderer consumes them.
> **wgpu (context7):** `/gfx-rs/wgpu` for API lookup; `/websites/sotrh_github_io_learn-wgpu` for design rationale.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md) §4.1, §4.3

---

## 1. Frame Structure

Each frame runs five stages in fixed order.

| Stage | Work |
|-------|------|
| **Input** | Poll events, update input state |
| **Game logic** | Fixed-timestep update: entity movement, collision, game rules |
| **Audio** | Update listener position, trigger sounds from game events |
| **Render** | Determine visible set, draw visible geometry, dynamic lights, sprites, post-processing |
| **Present** | Swap buffers |

Game logic runs at a fixed timestep decoupled from render rate. Renderer interpolates between the last two game states for smooth visuals at variable framerates. Simulation is deterministic at any refresh rate.

**View vs. sim split.** View angles (yaw, pitch) update at render rate from raw input; player position updates inside the fixed-tick loop and is interpolated between tick states. Evanescent inputs (mouse delta) are consumed at render rate so they are never lost on zero-tick frames. See `input.md §3`.

---

## 2. Visibility and Traversal

Visibility is computed per frame from baked portal geometry — the id Tech 4 approach. Precomputed PVS lengthens compile cycles and fights dynamic geometry; per-frame portal traversal is cheap at modern leaf counts.

Portal traversal is the primary path. The `--pvs` fallback is deprecated; do not extend it.

**Portal traversal.** CPU flood-fill. At each portal, clip the portal polygon against the current frustum. A non-empty clip result confirms visibility and narrows the frustum for the next hop. Produces a visible-cell bitmask consumed by the BVH traversal compute pass (§5).

**Fallback paths.** PVS, missing visibility data, and exterior-camera cases all fall back to per-leaf AABB frustum culling. See `build_pipeline.md` §Runtime visibility for the compile-side picture.

---

## 3. Level Loading

Loader parses PRL via the `postretro-level-format` crate. Uploads the global vertex/index buffer and BVH arrays to GPU storage buffers. Matches PNG textures by name (checkerboard placeholder for missing albedo, neutral normal for missing normal map). Renderer performs all GPU uploads and returns opaque handles — raw PRL types never cross into renderer code.

---

## 4. Lighting

Two components: **dynamic direct illumination** and **baked indirect illumination**. Both evaluated per fragment in the world shader — no deferred stages, no lightmap atlas.

**Direct.** Up to 500 authored lights per level. Fragment shader loops over all active lights with an influence-volume early-out. Shadow-casting lights draw from a fixed slot pool; a CPU visibility test each frame assigns slots by distance priority. Light sources: FGD entities (`light`, `light_spot`, `light_sun`) and gameplay effects. Clustered forward+ binning is deferred until profiling shows the flat loop bottlenecks.

**Indirect.** prl-build bakes an SH L2 irradiance volume (3D probe grid) over the level's empty space. Runtime samples via trilinear interpolation per fragment. Missing probe section falls back to the ambient floor.

**Normal maps.** Perturb the per-fragment normal before direct and indirect evaluation. Tangents baked into the vertex format at compile time.

**Light authoring.** Mappers place light entities in TrenchBroom. Compiler translates FGD properties to a canonical internal format with validation (falloff distance, spotlight direction, intensity bounds). Canonical lights feed both the SH baker and the runtime direct path. See `build_pipeline.md` §Custom FGD.

---

## 5. Cells, BVH, and Draw Leaves

**Cell** = opaque visibility unit. One cell per empty BSP leaf.

World geometry is organized into a global BVH at compile time. Each BVH leaf covers one `(face, material_bucket)` pair. Leaves are sorted by material bucket so each bucket owns a contiguous slot range in the indirect buffer.

**Draw flow.** Portal traversal (§2) produces a visible-cell bitmask → BVH traversal compute walks the tree, tests each leaf AABB and its cell bit, writes or zeros the leaf's indirect buffer slot → opaque pass issues one `multi_draw_indexed_indirect` call per material bucket against its contiguous slot range.

**Global vs. per-region.** One BVH over all static geometry. Global wins on shader simplicity and tree quality. Per-region is the pivot path if a cell-heavy map regresses on frame time — tighter cache behavior at the cost of more bookkeeping and storage buffers. Pivot only when global is measured to fall short. No hardware ray tracing — not in baseline wgpu.

---

## 6. Vertex Format

Custom format for all world geometry. Non-position attributes are quantized where precision loss is imperceptible at the target aesthetic.

| Attribute | Purpose |
|-----------|---------|
| Position | Geometry placement |
| Base UV | Diffuse and normal-map texture sampling |
| Normal | Per-fragment shading normal |
| Tangent | Tangent-space basis for normal-map sampling |

UVs computed from face projection data at compile time; GPU sampler uses repeat addressing. Normals and tangents use octahedral encoding — half the storage of a full float vector at visually-indistinguishable precision. Both generated in prl-build. No per-vertex lighting channel — direct and indirect both accumulate per fragment (§4).

---

## 7. Rendering Stages

### 7.1 Visibility and Culling Prepasses

1. **Portal traversal** (CPU) — §2 flood-fill produces the visible-cell bitmask.
2. **BVH traversal** (compute) — walks the global BVH; tests each leaf AABB against the frustum and the leaf's cell bit; writes or zeros the leaf's indirect buffer slot.
3. **Light list upload** — uploads the active dynamic light array and per-light influence volumes to GPU storage buffers.

### 7.2 World Geometry

Single opaque pass. One `multi_draw_indexed_indirect` call per material bucket. Per-fragment:

- Sample albedo and normal map; reconstruct world-space normal from TBN and normal-map sample.
- Sample SH irradiance volume (trilinear) for indirect lighting.
- Loop over active lights; evaluate direct contribution and sample shadow map.
- Output: `albedo × (indirect_sh + Σ direct_lights)`.

Depth testing and back-face culling are permanent from this pass forward. Shadow maps, billboards, emissive bypass, fog volumes, and post-processing attach in later phases.

---

## 8. Boundary Rule

All wgpu calls live in the renderer module. Map loader, game logic, audio, and input never import wgpu types. Data crosses the boundary as engine-defined types; the renderer translates to GPU operations. Per-subsystem contracts: vertex format §6, cells and BVH §5, lighting §4.

---

## 9. Camera

### Coordinate System

Right-handed, Y-up. Forward is −Z. Matches glam defaults and wgpu NDC.

### Projection Defaults

| Parameter | Default | Rationale |
|-----------|---------|-----------|
| Horizontal FOV | 100° | Modern boomer shooter default. Configurable 60°–130°. Vertical FOV derived from aspect ratio. |
| Near clip | 0.1 units | Close enough for weapon models without z-fighting |
| Far clip | 4096.0 units | Covers the full coordinate range for large maps |
| Aspect ratio | Derived from window | Updated on window resize |

### View Matrix

Camera position and orientation produce a view matrix each frame, feeding:

- Visibility (§2) — camera position seeds the portal flood-fill or PVS lookup
- Frustum culling — view-projection matrix defines the clip volume
- All draw calls — view-projection uniform uploaded once per frame

---

## 10. Non-Goals

- **Deferred rendering** — forward lighting with influence-volume early-out scales to the 500-light target. Indoor portal-isolated geometry keeps per-fragment light iteration cheap. Deferred adds complexity without benefit.
- **Baked lightmaps** — indirect lighting lives in the SH irradiance volume. No lightmap atlas.
- **PBR materials** — albedo + normal map is the full material vocabulary. Metallic/roughness is out of scope.
- **Hardware ray tracing** — not in baseline wgpu. Shadow maps cover dynamic shadowing; SH volume covers indirect.
- **Mesh shaders** — not baseline in wgpu. GPU-driven culling uses compute + `draw_indexed_indirect`.
- **Runtime level compilation** — maps compiled offline by prl-build. Engine is a consumer only.
- **Multiplayer / networking** — single-player engine. Out of project scope.
