# Rendering Pipeline

> **Read this when:** implementing or modifying the renderer, level loading, lighting, or any visual pass.
> **Key invariant:** renderer owns all wgpu calls. Other subsystems never touch GPU types. Level loaders produce handles; renderer consumes them.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md) §4.1, §4.3

---

## 1. Frame Structure

Each frame runs five stages in fixed order. Later stages depend on results from earlier ones.

| Stage | Work |
|-------|------|
| **Input** | Poll events, update input state |
| **Game logic** | Fixed-timestep update: entity movement, collision, game rules |
| **Audio** | Update listener position, trigger sounds from game events |
| **Render** | Determine visible set (leaves via portal traversal), draw visible geometry, dynamic lights, sprites, post-processing |
| **Present** | Swap buffers |

Game logic runs at a fixed timestep decoupled from render rate. Renderer interpolates between the last two game states for smooth visuals at variable framerates. Simulation is deterministic at any refresh rate. Rendering never blocks or drives the simulation clock.

**View vs. sim split.** View rotation (yaw, pitch) updates at render rate — once per frame, from mouse displacement and gamepad look velocity, before the fixed-tick loop. Player position updates inside the tick loop and is interpolated between tick states; view angles bypass interpolation and are read directly from the camera at render time, since they already update every frame. This mirrors id Tech 3's architecture: client viewangles update per frame; simulation ticks at a fixed rate. Evanescent inputs (mouse delta) are consumed at render rate so they are never lost on zero-tick frames. See `input.md §3`.

**Edge cases:** On the first frame, only one game state exists — duplicate it so interpolation produces the initial state with no blending. After a long stall (alt-tab, disk I/O), clamp the accumulator (e.g., 250ms max) to prevent dozens of catch-up ticks. On a stall catch-up (e.g., 5 ticks in one frame), view angles are updated once at render rate before the tick loop; all 5 ticks use the same freshest view direction. This is correct.

---

## 2. Visibility and Traversal

Visibility is **computed per frame from baked portal geometry**. This is the id Tech 4 (Doom 3, 2004) approach, not Quake 1's precomputed-PVS model — Carmack's reasoning for the break still applies: precomputed PVS lengthens compile cycles, fights with dynamic geometry, and per-frame portal traversal is trivially cheap at modern leaf counts.

Portals are the primary and forward path. The PRL `--pvs` fallback is **deprecated** and will be removed once portals are reliable on every supported map type. New feature work targets the portal-traversal path. Do not extend the deprecated path.

### PRL path (primary): runtime portal traversal

Single-pass portal flood-fill with clip-and-narrow at each hop. This is the id Tech 4 (Doom 3, 2004) form of runtime portal vis.

At each portal the flood-fill visits, the portal polygon is clipped against the current frustum. An empty clip result rejects the portal entirely. A non-empty clip result both confirms visibility and drives frustum narrowing: the new frustum is built from the portal plane and one edge plane per clipped edge through the camera position.

**Strict-subset invariant.** The clipped polygon lies entirely inside the current frustum by construction, so the edge planes derived from it form a cone strictly inside the current cone. By induction from the camera's initial frustum, every narrowed frustum reachable through any portal chain is a strict subset of the camera frustum. Every leaf marked visible by the flood-fill lies inside the camera's view cone.

There is no separate per-leaf AABB frustum cull on this path. The clip-and-narrow step both tests visibility and builds the next frustum in one operation, and the strict-subset invariant makes a second enforcement pass redundant. Solid leaves block traversal.

### PRL path (`--pvs` fallback)

**Status: deprecated.** Use only when portal generation cannot produce valid output for a map. Will be removed once portal generation is reliable on every supported map type. Do not extend.

When a PRL file was built with `--pvs`, the Portals section is absent and a precomputed PVS bitset replaces runtime portal traversal. The renderer descends to the camera leaf, looks up its PVS bitset, and draws every empty leaf in the bitset that survives per-leaf AABB frustum culling.

### Frustum culling

Per-leaf AABB frustum culling does not apply on the portal-traversal path: the strict-subset invariant guarantees every reached leaf already lies inside the camera's view cone. All other paths (PVS, no-PVS fallback, solid-leaf fallback, exterior-camera fallback) use per-leaf AABB culling to narrow the draw set before draw-range emission.

**Missing visibility data:** when neither portals nor PVS is present (PRL without a visibility section), draw all empty leaves with frustum culling only. Slower but correct.

**Camera outside playable space:** camera in exterior or solid leaf. Frustum-cull all interior leaves. Back-face culling hides the level shell — face winding is front-facing from inside, back-facing from outside. Same cull mode serves both cases.

See `build_pipeline.md` §Runtime visibility for the compile-side picture.

---

## 3. Level Loading Pipeline

Loader parses level data into engine-side structs, produces GPU-ready data. Renderer consumes handles, never raw level types. This boundary is strict: raw format types do not appear in renderer code.

**PRL path.** Loaded via the `postretro-level-format` crate. Pre-processed at compile time by `prl-build`, so the runtime load is mostly buffer hand-off and texture matching.

**Load sequence:**

1. Parse the level file into typed structures (vertices, faces, textures, visibility data, SH irradiance volume).
2. Build engine-side vertex data: positions (coordinate-transformed to engine Y-up), texture UVs, packed normals, packed tangents. PRL data is loaded directly. See §6.
3. Load PNG textures (albedo + normal map) matched by texture name strings. Generate checkerboard placeholders for missing albedo, neutral normals (0,0,1) for missing normal maps.
4. Build per-face metadata: material type (from texture name prefix), texture indices, draw command parameters.
5. Group faces into per-cell draw chunks keyed by portal cell; each chunk owns a contiguous index range and records its AABB for GPU culling.
6. Hand prepared data to the renderer. Renderer performs all GPU uploads — loader never calls wgpu.
7. Renderer returns opaque handles. All subsequent draw operations reference these handles.

---

## 4. Lighting

Lighting has two components: **dynamic direct illumination** (clustered forward+ with shadow maps) and **baked indirect illumination** (SH irradiance volume sampled per fragment). Both are evaluated in the world shader during the opaque geometry pass — no deferred stages, no lightmap atlas.

**Direct illumination.** Dynamic lights (point, spot, directional) are built into a clustered light list each frame by a compute prepass. The fragment shader reads the cluster for its screen-space tile and accumulates contributions from lights whose volume reaches that fragment. Shadow-casting lights write to shadow maps (cascaded shadow maps for directional, cube shadow maps for point and spot) before the main pass; the fragment shader samples them during accumulation. Light sources originate from FGD entities (`light`, `light_spot`, `light_sun`) and from gameplay effects (muzzle flashes, explosions).

**Indirect illumination.** prl-build bakes a regular 3D grid of SH L2 probes over the level's empty space, evaluating incoming radiance at each probe by raycasting against static geometry with canonical lights as sources. The runtime samples the probe grid via trilinear interpolation in the fragment shader. Missing probe section falls back to flat white ambient.

**Normal maps.** Tangent-space normal maps perturb the per-fragment normal before both direct and indirect evaluation. Tangents are packed into the vertex format (§6) at compile time.

**Light entity authoring.** Mappers place light entities in TrenchBroom. The compiler's translation layer converts mapper-facing FGD properties to an internal canonical format, applying validation rules (falloff distance required, spotlight direction verified, intensity bounds checked). Canonical lights feed both the SH baker and the runtime direct-lighting path. See `build_pipeline.md` §Custom FGD.

Full spec: `plans/drafts/phase-4-baked-lighting/`

---

## 5. Cells and Draw Chunks

**Cell** = empty BSP leaf in its draw-chunk and visibility role. 1:1 with leaves today; the separate term leaves room for future subdivision or merging without a spec rewrite. **Cluster** = screen-space light-culling grid (§7.1 step 4), never spatial. Rule: cell = world space, cluster = screen space.

World geometry is grouped by cell at compile time. Each chunk:

| Field | Content |
|-------|---------|
| `cell_id` | Portal-graph node |
| `aabb` | World-space bounds for GPU frustum + HiZ culling |
| `index_offset` | Start of the chunk's indices in the shared index buffer |
| `index_count` | Length of the index range |
| `material_bucket` | (albedo, normal map) pair the indices reference |

Indices within a cell are ordered by material bucket, so each cell emits one indirect draw per material it touches (typical: 2–10). The chunk table lives in its own PRL section; the loader hands it to the renderer.

Flow: portal traversal (§2) produces the visible cell list → compute cell-culling prepass (§7.1 step 3) frustum- and HiZ-culls → emits `draw_indexed_indirect` commands → opaque pass (§7.2) consumes via `multi_draw_indexed_indirect`, one call per material bucket.

---

## 6. Vertex Format

Custom vertex format used for all world geometry. Packed for cache efficiency — non-position attributes are quantized where the precision loss is imperceptible at the target aesthetic.

| Attribute | Content | Purpose |
|-----------|---------|---------|
| Position | `f32 × 3` world-space coordinate (Y-up, engine meters) | Geometry placement |
| Base UV | `f32 × 2` texture-space coordinate, normalized by texture dimensions | Diffuse and normal-map texture sampling |
| Normal | Octahedral-encoded `u16 × 2` | Per-fragment shading normal (pre-normal-map) |
| Tangent | Octahedral-encoded `u16 × 2` plus sign bit | Tangent-space basis for normal-map sampling |

UVs are computed from face projection data (s-axis, t-axis, offsets) during compilation. The GPU sampler uses repeat addressing — UVs outside [0, 1] tile correctly.

Normals and tangents are packed via octahedral encoding (two `u16` per vector), which preserves direction to visually-indistinguishable precision at half the storage of `f32 × 3`. The tangent's bitangent sign rides in a spare bit so the vertex shader can reconstruct the full TBN matrix. Both are generated at compile time in prl-build during the brush-side projection stage — normals from face plane, tangents from the UV projection axes.

The earlier per-vertex color channel is removed: dynamic light accumulation happens per fragment in the clustered shading pass (§4), not via per-vertex interpolation.

---

## 7. Rendering Stages

Clustered forward+ pipeline. Each frame runs a small set of compute prepasses that build culling and lighting state, then a single opaque geometry pass that consumes it, then post-processing.

### 7.1 Visibility and Culling Prepasses

1. **Portal traversal** (CPU) — §2 flood-fill produces the visible cell set.
2. **HiZ depth pyramid** (compute, *Phase 3.5*) — downsample the previous frame's depth buffer into a hierarchical-Z pyramid used for occlusion testing. First frame uses a permissive pass-all bound.
3. **GPU cell culling** (compute, *Phase 3.5*) — each surviving cell's AABB is tested against the current frustum and HiZ pyramid. Surviving cells emit `draw_indexed_indirect` commands into an indirect buffer, grouped by material bucket.
4. **Clustered light list** (compute, *Phase 4*) — builds per-cluster light index lists from the dynamic light set. Cluster grid is screen-space tiles × depth slices.

### 7.2 World Geometry

Single opaque pass. CPU issues one `multi_draw_indexed_indirect` call per material bucket against its slice of the indirect buffer built in §7.1 — typically 10–50 calls per frame. Collapsing to one call would need bindless descriptor arrays, not baseline in wgpu. Per-fragment shading:

- Sample base texture and normal map at the UV coordinate. Reconstruct world-space normal from the TBN and normal-map sample.
- Sample the SH L2 irradiance volume at fragment position (trilinear) for indirect lighting.
- Walk the fragment's cluster light list; for each light, evaluate direct contribution and sample the associated shadow map.
- Output = `albedo × (indirect_sh + Σ direct_lights)`.

Depth testing (Less, write enabled) and back-face culling (counter-clockwise front face) are permanent from this phase forward.

### 7.3 Shadow Maps

> **Phase 4.**

Shadow-casting dynamic lights render into dedicated depth targets before the opaque pass. Directional lights use cascaded shadow maps (CSM); point and spot lights use cube or single shadow maps respectively. Resolution is intentionally modest — chunky pixel shadow edges match the target aesthetic.

### 7.4 Billboard Sprites

> **Phase 5+. Not yet implemented.**

Camera-facing textured quads for characters, pickups, and decorative elements. Classic Doom-style billboarding. Lit by the same SH irradiance volume as world geometry, plus any reaching dynamic lights.

### 7.5 Emissive / Fullbright Surfaces

> **Rendering behavior Phase 5+.** Material flag is derived and stored during level load. The rendering bypass is not yet implemented.

Neon signs, screens, glowing panels: bypass lighting modulation, render at full brightness. Identified by the emissive flag on the material enum variant. See `resource_management.md` §3.

### 7.6 Fog Volumes

> **Phase 5+. Not yet implemented.**

Per-volume fog via `env_fog_volume` brush entities. Resolved to BSP leaves at load time. Per-fragment effect — not a screen-space post-process. Camera's current leaf determines the active fog volume. Smallest volume wins when a leaf belongs to multiple. See `audio.md` §6 for the same rule applied to reverb zones.

### 7.7 Post-Processing

> **Phase 6. Not yet implemented.**

| Effect | Description |
|--------|-------------|
| **Bloom** | Bright pixels bleed into surrounding area. Reinforces neon cyberpunk aesthetic. |
| **Tonemapping** | HDR lighting accumulation collapsed to display range. |
| **CRT / Scanline** | Optional retro display effects: scanlines, curvature, color fringing. Off by default. |

---

## 8. Data Contracts

### Map loader produces

| Output | Description |
|--------|-------------|
| Vertex buffer | Face vertices in custom vertex format (§6); grouped by portal cell |
| Index buffer | Triangle indices; contiguous per cell for indirect draws |
| Loaded textures | CPU-side data per texture (albedo + normal map), indexed by texture index. Checkerboard for missing albedo; neutral normal for missing normal map. |
| Per-face metadata | Material type, texture indices, index range within its cell's chunk |
| Per-cell draw chunks | `(cell_id, aabb, index_offset, index_count, material_bucket)` tuples consumed by GPU culling and indirect draw emission |
| SH irradiance volume | 3D grid of SH L2 coefficients (27 f32 per probe) plus validity mask |
| Canonical lights | Validated `light` / `light_spot` / `light_sun` entities for the runtime direct lighting path |

### Renderer consumes

| Input | Description |
|-------|-------------|
| GPU buffer handles | Vertex buffer, index buffer, per-cell chunk buffer, indirect draw buffer — opaque handles, not raw data |
| Material bind groups | One wgpu bind group per unique (albedo, normal map) pair |
| Per-frame uniforms | View-projection matrix, camera position, time, cluster grid parameters |
| SH volume texture | 3D texture storing interpolated SH coefficients; sampled per fragment |
| Shadow map atlas / cube array | Dynamic-light shadow targets written each frame before the opaque pass |

### Boundary rule

All wgpu calls live in the renderer module. Map loader, game logic, audio, and input never import wgpu types. Data crosses the boundary as engine-defined types; the renderer translates to GPU operations.

---

## 9. Camera

Projection and view parameters for rendering and visibility.

### Coordinate System

Right-handed, Y-up. Matches glam's default conventions and wgpu's NDC expectations. Forward is -Z (camera looks down the negative Z axis in view space).

### Projection Defaults

| Parameter | Default | Rationale |
|-----------|---------|-----------|
| Horizontal FOV | 100° | Modern boomer shooter default. Configurable 60°–130°. Vertical FOV derived from aspect ratio. |
| Near clip | 0.1 units | Close enough for weapon models without z-fighting artifacts |
| Far clip | 4096.0 units | Covers the full coordinate range for large maps |
| Aspect ratio | Derived from window dimensions | Updated on window resize |

### View Matrix

Camera position and orientation produce a view matrix each frame. The view matrix feeds:

- Visibility (§2) — camera position seeds the portal-traversal flood-fill (default) or the PVS lookup (`--pvs` fallback)
- Frustum culling — view-projection matrix defines the clip volume
- All draw calls — view-projection uniform uploaded once per frame

---

## 10. Non-Goals

- **Deferred rendering** — clustered forward+ is sufficient for the target light count and aesthetic. Deferred adds complexity without benefit here.
- **Baked lightmaps** — indirect lighting lives in the SH irradiance volume. No lightmap atlas, no per-face lightmap UVs, no lightmap bake stage.
- **PBR materials** — albedo + normal map is the full material vocabulary. Metallic/roughness workflows are out of scope.
- **Hardware ray tracing** — not available in baseline wgpu. Shadow maps cover dynamic shadowing; the SH volume covers indirect illumination.
- **Mesh shaders** — not baseline in wgpu. GPU-driven culling uses compute + `draw_indexed_indirect` instead.
- **Runtime level compilation** — maps are compiled offline by prl-build. The engine is a consumer, not a compiler.
- **Multiplayer / networking** — single-player engine. Network synchronization is not a rendering concern and is excluded from the project scope entirely.
