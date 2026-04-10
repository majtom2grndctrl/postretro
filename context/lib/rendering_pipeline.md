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
| **Render** | Determine visible set (BSP leaves via portal traversal), draw visible geometry, dynamic lights, sprites, post-processing |
| **Present** | Swap buffers |

Game logic runs at a fixed timestep decoupled from render rate. Renderer interpolates between the last two game states for smooth visuals at variable framerates. Simulation is deterministic at any refresh rate. Rendering never blocks or drives the simulation clock.

**Edge cases:** On the first frame, only one game state exists — duplicate it so interpolation produces the initial state with no blending. After a long stall (alt-tab, disk I/O), clamp the accumulator (e.g., 250ms max) to prevent dozens of catch-up ticks.

---

## 2. Visibility and Traversal

Visibility is **computed per frame from baked portal geometry**. This is the id Tech 4 (Doom 3, 2004) approach, not Quake 1's precomputed-PVS model — Carmack's reasoning for the break still applies: precomputed PVS lengthens compile cycles, fights with dynamic geometry, and per-frame portal traversal is trivially cheap at modern leaf counts. PRL builds always include portal geometry by default; precomputed PVS exists as a `--pvs` fallback.

### PRL path (primary): runtime portal traversal

Two cooperating layers determine which leaves are drawn each frame. Both are load-bearing.

1. **Portal flood-fill** seeded at the camera leaf. Walks the portal graph outward; at each portal, tests the polygon against the current frustum and narrows the frustum through it. Solid leaves block traversal. Yields a per-leaf reachability bitset.
2. **Per-leaf AABB frustum cull** drops any reached leaf whose bounding box lies entirely outside the camera's view frustum.

The second layer is the corrective for the first. Frustum narrowing inside the flood-fill is approximate: portal-edge planes constrain sideways visibility through each portal but the camera's original side planes are not carried through narrowing. Reachable leaves can sit outside the camera's view cone, particularly when portals straddle the cone boundary. The AABB cull restores camera-cone enforcement coarsely at draw-range emission.

The two-layer hybrid is informed-but-imperfect rather than the architectural ideal. The id Tech 4 algorithm clips the portal polygon against the current frustum before narrowing, which keeps each narrowed frustum a strict subset of the camera frustum and folds AABB enforcement into the recursion. Adopting that upgrade would let the AABB pass be removed cleanly. See `plans/drafts/portal-polygon-clipping/` for the planned migration.

**Do not remove either layer in isolation.** Removing the AABB cull without first making the flood-fill produce strict-subset frustums causes runtime culling to silently degrade — every leaf reached through any portal chain gets drawn regardless of whether it lies in the camera's view cone.

### PRL path (`--pvs` fallback)

When a PRL file was built with `--pvs`, the Portals section is absent and a precomputed PVS bitset replaces runtime portal traversal. The renderer descends to the camera leaf via BSP traversal, looks up the leaf's PVS bitset, and draws every empty leaf in the bitset that survives per-leaf AABB frustum culling. Slower compile, faster runtime — preserved as a fallback for build configurations that prefer compile-time cost over per-frame work.

### BSP path (legacy support)

`.bsp` files compiled by ericw-tools carry precomputed PVS only — no portal data. Visibility uses the same precomputed-PVS-then-AABB-frustum approach as the PRL `--pvs` fallback. No active development on this path.

### Frustum culling

Per-leaf AABB frustum culling is the final filter before draw-range emission in every path. It is not a separate optimization layered on top of visibility — it is part of the visibility decision. Load-bearing in the PRL primary path (it restores camera-cone enforcement after the flood-fill) and essential to the PVS fallback paths (PVS is conservative; frustum culling tightens it).

**Missing visibility data:** when neither portals nor PVS is present (corrupted BSP, PRL without a visibility section), draw all empty leaves with frustum culling only. Slower but correct.

See `build_pipeline.md` §Runtime visibility for the compile-side picture.

---

## 3. Level Loading Pipeline

Loader parses level data into engine-side structs, produces GPU-ready data. Renderer consumes handles, never raw level types. This boundary is strict: raw format types do not appear in renderer code.

The load sequence below applies to both PRL (primary) and BSP (legacy). BSP loading uses the qbsp crate; PRL loading uses the postretro-level-format crate. Both produce the same engine-side types consumed by the renderer.

**Load sequence:**

1. Parse BSP file via qbsp. Typed access to vertices, edges, faces, textures, and visibility data.
2. Build engine-side vertex data: positions (coordinate-transformed to engine Y-up), texture UVs (computed from BSP face projection data), vertex color (white default). See §6.
3. Load PNG textures matched by BSP texture name strings. Generate checkerboard placeholders for missing textures.
4. Build per-face metadata: material type (from texture name prefix), texture index, draw command parameters.
5. Sort faces by (leaf, texture) for draw batching. Pre-compute per-leaf texture sub-ranges.
6. Hand prepared data to the renderer. Renderer performs all GPU uploads — loader never calls wgpu.
7. Renderer returns opaque handles. All subsequent draw operations reference these handles.

---

## 4. Phase 4 Lighting (Planned)

> **Phase 4. Not yet designed.** Flat white ambient lighting is the current state. Phase 4 will add baked lighting baked into PRL by prl-build — not via BSPX lumps.

Lighting data will live in PRL sections produced by the compiler. The exact mechanism (lightmaps, light probes, irradiance volumes) is a Phase 4 design decision. Desired properties to preserve from the aspirational design:

- Per-face colored lighting with directional component for approximate specular
- Ambient occlusion baked into lightmap samples
- Volumetric light probes for dynamic object lighting (sprites, particles)

Missing lighting data is not an error. Current fallback — flat white ambient — remains the default until Phase 4 ships.

---

---

## 6. Vertex Format

Custom vertex format used for all BSP world geometry.

| Attribute | Content | Purpose |
|-----------|---------|---------|
| Position | 3D world-space coordinate (Y-up, engine meters) | Geometry placement |
| Base UV | Texture-space coordinate, normalized by texture dimensions | Diffuse texture sampling |
| Vertex color | RGBA per-vertex tint (white default) | Dynamic lighting accumulation (Phase 5+) |

UVs are computed from BSP face projection data (s-axis, t-axis, offsets) during load. The GPU sampler uses repeat addressing — UVs outside [0, 1] tile correctly.

Vertex color carries per-vertex lighting contributions in later phases. Currently unused beyond providing a tint channel (white = no effect). Phase 5 adds dynamic light accumulation.

---

## 7. Rendering Stages

Forward rendering pipeline. Each stage runs as a distinct render pass or draw call group within a frame.

### 7.1 BSP World Geometry

Draw visible faces from the visibility-culled draw set (§2). Draw calls grouped by (leaf, texture) — one call per visible leaf × texture pair. Minimizes bind group switches without breaking leaf contiguity required by visibility tracking.

Each face samples its base texture at its UV coordinate. Flat ambient lighting applied uniformly: `output = base_texture × ambient_light × vertex_color`. Phase 4 replaces the flat ambient factor with probe-sampled per-surface values.

Depth testing (Less, write enabled) and back-face culling (counter-clockwise front face) are permanent from this phase forward.

### 7.2 Dynamic Lights

> **Phase 5+. Not yet implemented.**

Forward point lights supplementing baked lighting: muzzle flashes, neon glow, explosions, projectile trails. Transient, gameplay-driven illumination — not a replacement for baked lighting. Accumulate into vertex color or evaluate per-fragment.

### 7.3 Billboard Sprites

> **Phase 5+. Not yet implemented.**

Camera-facing textured quads for characters, pickups, and decorative elements. Classic Doom-style billboarding. Lit by nearest light probe from Phase 4 lighting data; fallback to flat ambient when absent.

### 7.4 Emissive / Fullbright Surfaces

> **Rendering behavior Phase 5+.** Material flag is derived and stored during level load. The rendering bypass is not yet implemented.

Neon signs, screens, glowing panels: bypass lighting modulation, render at full brightness. Identified by the emissive flag on the material enum variant. See `resource_management.md` §3.

### 7.5 Fog Volumes

> **Phase 5+. Not yet implemented.**

Per-volume fog via `env_fog_volume` brush entities. Resolved to BSP leaves at load time. Per-fragment effect — not a screen-space post-process. Camera's current leaf determines the active fog volume. Smallest volume wins when a leaf belongs to multiple. See `audio.md` §6 for the same rule applied to reverb zones.

### 7.6 Post-Processing

> **Phase 5+. Not yet implemented.**

| Effect | Description |
|--------|-------------|
| **Bloom** | Bright pixels bleed into surrounding area. Reinforces neon cyberpunk aesthetic. |
| **CRT / Scanline** | Optional retro display effects: scanlines, curvature, color fringing. Off by default. |

---

## 8. Data Contracts

### BSP loader produces

| Output | Description |
|--------|-------------|
| Vertex buffer | BSP face vertices in custom vertex format (§6); sorted by (leaf, texture) |
| Index buffer | Triangle indices; sorted by (leaf, texture) for draw batching |
| Loaded textures | CPU-side RGBA8 data per texture, indexed by BSP texture index. Checkerboard for missing. |
| Per-face metadata | Material type, texture index, draw command parameters (index offset, index count) |
| Per-leaf texture sub-ranges | Pre-computed (texture_index, index_offset, index_count) tuples per leaf for the draw loop |

### Renderer consumes

| Input | Description |
|-------|-------------|
| GPU buffer handles | Vertex buffer, index buffer — opaque handles, not raw data |
| Per-texture bind groups | One wgpu bind group per unique texture (texture view + sampler) |
| Per-frame uniform | View-projection matrix, ambient light factor |
| Per-leaf texture sub-ranges | Drive the draw loop: one indexed draw call per (visible_leaf, texture) pair |

### Boundary rule

All wgpu calls live in the renderer module. BSP loader, game logic, audio, and input never import wgpu types. Data crosses the boundary as engine-defined types; the renderer translates to GPU operations.

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
| Far clip | 4096.0 units | Covers the full BSP2 coordinate range for large maps |
| Aspect ratio | Derived from window dimensions | Updated on window resize |

### View Matrix

Camera position and orientation produce a view matrix each frame. The view matrix feeds:

- Visibility (§2) — camera position seeds the portal-traversal flood-fill (PRL primary) or the PVS lookup (`--pvs` fallback / BSP legacy)
- Frustum culling — view-projection matrix defines the clip volume
- All draw calls — view-projection uniform uploaded once per frame

---

## 10. Non-Goals

- **Deferred rendering** — forward pipeline is sufficient for the target light count and aesthetic. Deferred adds complexity without benefit here.
- **Runtime level compilation** — maps are compiled offline (ericw-tools for BSP, prl-build for PRL). The engine is a consumer, not a compiler.
- **PBR materials** — baked lightmaps and simple Blinn-Phong specular achieve the retro aesthetic. Metallic/roughness workflows are out of scope.
- **Ray tracing** — baked lighting plus a small number of dynamic lights covers the visual needs.
- **Multiplayer / networking** — single-player engine. Network synchronization is not a rendering concern and is excluded from the project scope entirely.
