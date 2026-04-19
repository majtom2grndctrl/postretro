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

Three components: **static direct** (baked), **dynamic direct** (runtime), and **indirect** (baked). All evaluated per fragment in the world shader — no deferred stages.

**Static direct.** prl-build UV-unwraps world geometry and ray-casts per-texel irradiance and a dominant incoming light direction from all static lights into a directional lightmap atlas. Runtime samples the atlas per fragment with nearest-neighbor filtering on both irradiance and direction textures — hard-edged pixelated shadows match the retro aesthetic, and nearest is arguably more correct on octahedral-encoded directions (linear interpolation doesn't commute with slerp). Bumped-Lambert correction preserves normal-map response to baked static lights. Hard shadows from static lights are captured in the bake.

**Dynamic direct.** Dynamic lights run a per-fragment loop with an influence-volume early-out. No runtime shadow maps for dynamic lights in this iteration. Light sources: FGD entities (`light`, `light_spot`, `light_sun`) and gameplay effects. Clustered forward+ binning deferred until profiling shows the flat loop bottlenecks.

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
| Lightmap UV | Static direct lighting atlas sampling |

UVs computed from face projection data at compile time; GPU sampler uses repeat addressing. Normals and tangents use octahedral encoding — half the storage of a full float vector at visually-indistinguishable precision. Both generated in prl-build. No per-vertex lighting channel — direct and indirect both accumulate per fragment (§4).

---

## 7. Rendering Stages

### 7.1 Visibility and Culling Prepasses

1. **Portal traversal** (CPU) — §2 flood-fill produces the visible-cell bitmask.
2. **BVH traversal** (compute) — walks the global BVH; tests each leaf AABB against the frustum and the leaf's cell bit; writes or zeros the leaf's indirect buffer slot.
3. **Light list upload** — uploads the active dynamic light array and per-light influence volumes to GPU storage buffers.

### 7.2 Depth Pre-Pass

Vertex-only pass over the same indirect draw list as the forward pass. Runs the same view-projection transform and writes depth only — no fragment stage. Populates the shared depth buffer so the forward pass can eliminate overdraw.

Both the depth pre-pass and the forward vertex shader declare `@invariant` on `clip_position`. Without it, some GPUs reassociate the `mat4 × vec4` multiply differently across pipelines, producing Z-fighting dropout when the forward pass tests `Equal`.

### 7.3 World Geometry

One `multi_draw_indexed_indirect` call per material bucket. Depth loaded from the pre-pass buffer (`LoadOp::Load`); depth compare is `Equal`, depth writes disabled — each fragment is shaded exactly once. Per-fragment:

- Sample albedo and normal map; reconstruct world-space normal from TBN and normal-map sample.
- Sample lightmap atlas (irradiance + dominant direction); apply bumped-Lambert correction for normal-map response to static lights.
- Sample SH irradiance volume (trilinear) for indirect lighting.
- Loop over dynamic lights; evaluate direct contribution with influence-volume early-out.
- Output: `albedo × (static_direct + indirect_sh + Σ dynamic_direct)`.

Depth testing and back-face culling are permanent from this pass forward.

### 7.4 Billboard Sprite Pass

Camera-facing quads emitted from `env_smoke_emitter` point entities. Alpha-blended additive pass; depth write disabled, depth test enabled. Quads are expanded in the vertex shader using the view-space right and up vectors — no geometry shader. Lit by the full stack: SH ambient, multi-source static specular via the chunk light list, and dynamic direct (diffuse only). Batched by sprite-sheet collection — all emitters sharing a collection issue one draw call per frame. Bind group 6 carries the sprite instance storage buffer.

### 7.5 Fog Volume Composite

Low-resolution raymarched pass over `env_fog_volume` brush regions. Resolution governed by `fog_pixel_scale` worldspawn property (default 4 — quarter resolution). Per sample: point-in-AABB membership test against the fog volume buffer; accumulates SH ambient scatter and dynamic spot beam scatter (with shadow map occlusion for visible shafts and shadow wedges). Composited over the scene additively via nearest-neighbor upscale. The pixelated blocks are intentional, not a compromise.

---

## 8. Boundary Rule

All wgpu calls live in the renderer module. Map loader, game logic, audio, and input never import wgpu types. Data crosses the boundary as engine-defined types; the renderer translates to GPU operations. Per-subsystem contracts: vertex format §6, cells and BVH §5, lighting §4.

**Device limits.** Renderer requests `max_bind_groups = 8` — the WebGPU spec maximum and the ceiling for any future pass. Allocated bind-group slots:

| Group | Contents |
|-------|---------|
| 0 | Camera uniforms |
| 1 | Material (albedo texture, normal map, per-material uniforms) |
| 2 | Dynamic lights, influence volumes, per-chunk static light lists |
| 3 | SH irradiance volume (sampler, 9 coefficient band textures, grid uniform) |
| 4 | Lightmap atlas (irradiance + dominant direction textures) |
| 5 | Spot shadow maps (depth texture array, comparison sampler, light-space matrices) |
| 6 | FX resources (sprite instance storage buffer; fog depth buffer, AABB buffer, scatter target) |

Groups 0, 2, 3, and 5 are shared across the forward, billboard, and fog pipelines — the same bind-group objects are reused, not re-uploaded. One budget slot remains; a pass needing a ninth group must consolidate, not raise the limit.

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

## 10. Diagnostics

### GPU Pass Timing

Set `POSTRETRO_GPU_TIMING=1` to enable per-pass GPU timing. Requires adapter support for `TIMESTAMP_QUERY`; silently disabled if the feature is absent. Passes measured: `cull`, `depth_prepass`, `forward`. Results are averaged over a 120-frame window and logged via `log::info!` at the window boundary. Use with `RUST_LOG=info` to see output.

---

## 11. Non-Goals

- **Deferred rendering** — forward lighting with influence-volume early-out scales to the 500-light target. Indoor portal-isolated geometry keeps per-fragment light iteration cheap. Deferred adds complexity without benefit.
- **PBR materials** — albedo + normal map is the full material vocabulary. Metallic/roughness is out of scope.
- **Hardware ray tracing** — not in baseline wgpu. Shadow maps cover dynamic shadowing; SH volume covers indirect.
- **Mesh shaders** — not baseline in wgpu. GPU-driven culling uses compute + `draw_indexed_indirect`.
- **Runtime level compilation** — maps compiled offline by prl-build. Engine is a consumer only.
- **Multiplayer / networking** — single-player engine. Out of project scope.
