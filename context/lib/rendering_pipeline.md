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

1. Parse the level file into typed structures (vertices, faces, textures, visibility data).
2. Build engine-side vertex data: positions (coordinate-transformed to engine Y-up), texture UVs, vertex color (white default). PRL data is loaded directly. See §6.
3. Load PNG textures matched by texture name strings. Generate checkerboard placeholders for missing textures.
4. Build per-face metadata: material type (from texture name prefix), texture index, draw command parameters.
5. Sort faces by (leaf, texture) for draw batching. Pre-compute per-leaf texture sub-ranges.
6. Hand prepared data to the renderer. Renderer performs all GPU uploads — loader never calls wgpu.
7. Renderer returns opaque handles. All subsequent draw operations reference these handles.

---

## 4. Phase 4 Lighting

> **Baked light entities.** Mappers author light entities in TrenchBroom (`light`, `light_spot`, `light_sun`). Compiler translates them to canonical format, validates them, and hands canonical lights to Phase 4.5 baker.

Light entities are defined in the FGD alongside fog and reverb entities (see `build_pipeline.md` §Custom FGD). The compiler's translation layer converts mapper-facing properties (light intensity, color, falloff distance, animation) to engine-internal canonical format, applying validation rules: falloff distance required, spotlight direction verified, intensity bounds checked. Invalid lights fail compilation with clear error messages.

**Phase boundary:** Phase 4 owns entity spec, translation layer, and validation. Phase 4.5 owns probe sampling strategy, baker algorithm, and renderer integration. Missing lighting section is not an error; flat white ambient remains the fallback until Phase 4.5 ships.

Full spec: `plans/drafts/phase-4-fgd-light-entities/`

---

## 6. Vertex Format

Custom vertex format used for all world geometry.

| Attribute | Content | Purpose |
|-----------|---------|---------|
| Position | 3D world-space coordinate (Y-up, engine meters) | Geometry placement |
| Base UV | Texture-space coordinate, normalized by texture dimensions | Diffuse texture sampling |
| Vertex color | RGBA per-vertex tint (white default) | Dynamic lighting accumulation (Phase 5+) |

UVs are computed from face projection data (s-axis, t-axis, offsets) during compilation. The GPU sampler uses repeat addressing — UVs outside [0, 1] tile correctly.

Vertex color carries per-vertex lighting contributions in later phases. Currently unused beyond providing a tint channel (white = no effect). Phase 5 adds dynamic light accumulation.

---

## 7. Rendering Stages

Forward rendering pipeline. Each stage runs as a distinct render pass or draw call group within a frame.

### 7.1 World Geometry

Draw visible faces from the visibility-culled draw set (§2). Draw calls grouped by (leaf, texture) — one call per visible leaf × texture pair. Minimizes bind group switches without breaking leaf contiguity required by visibility tracking.

Each face samples its base texture at its UV coordinate. Flat ambient lighting applied uniformly: `output = base_texture × ambient_light × vertex_color`. Phase 4.5 replaces flat ambient with per-probe illumination baked from Phase 4 light entities.

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

### Map loader produces

| Output | Description |
|--------|-------------|
| Vertex buffer | Face vertices in custom vertex format (§6); sorted by (leaf, texture) |
| Index buffer | Triangle indices; sorted by (leaf, texture) for draw batching |
| Loaded textures | CPU-side RGBA8 data per texture, indexed by texture index. Checkerboard for missing. |
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

- **Deferred rendering** — forward pipeline is sufficient for the target light count and aesthetic. Deferred adds complexity without benefit here.
- **Runtime level compilation** — maps are compiled offline by prl-build. The engine is a consumer, not a compiler.
- **PBR materials** — baked lightmaps and simple Blinn-Phong specular achieve the retro aesthetic. Metallic/roughness workflows are out of scope.
- **Ray tracing** — baked lighting plus a small number of dynamic lights covers the visual needs.
- **Multiplayer / networking** — single-player engine. Network synchronization is not a rendering concern and is excluded from the project scope entirely.
