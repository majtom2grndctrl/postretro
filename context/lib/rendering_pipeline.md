# Rendering Pipeline

> **Read this when:** implementing or modifying the renderer, BSP loader, lighting, or any visual pass.
> **Key invariant:** renderer owns all wgpu calls. Other subsystems never touch GPU types. BSP loader produces handles; renderer consumes them.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md) §4.1, §4.3

---

## 1. Frame Structure

Each frame runs five stages in fixed order. Later stages depend on results from earlier ones.

| Stage | Work |
|-------|------|
| **Input** | Poll events, update input state |
| **Game logic** | Fixed-timestep update: entity movement, collision, game rules |
| **Audio** | Update listener position, trigger sounds from game events |
| **Render** | Traverse BSP, draw visible geometry, dynamic lights, sprites, post-processing |
| **Present** | Swap buffers |

Game logic runs at a fixed timestep decoupled from render rate. Renderer interpolates between the last two game states for smooth visuals at variable framerates. Simulation is deterministic at any refresh rate. Rendering never blocks or drives the simulation clock.

**Edge cases:** On the first frame, only one game state exists — duplicate it so interpolation produces the initial state with no blending. After a long stall (alt-tab, disk I/O), clamp the accumulator (e.g., 250ms max) to prevent dozens of catch-up ticks.

---

## 2. BSP Visibility and Traversal

Visibility is determined per-frame using Potentially Visible Set (PVS) data baked into the BSP file by the compiler.

**Per-frame sequence:**

1. Determine which BSP leaf contains the camera position.
2. Look up the PVS for that leaf — a compressed bitfield of which other leaves are potentially visible.
3. Decompress the PVS into a visible leaf set.
4. Collect all faces belonging to visible leaves.
5. Submit collected faces as the frame's draw set.

PVS culling is conservative: it may include faces that are technically occluded, but never excludes a visible face. This is a solved trade-off — slight overdraw is cheaper than per-face occlusion tests.

Frustum culling can further reduce the draw set by discarding faces outside the camera's view volume. PVS runs first (coarse), frustum culling runs second (fine).

**Missing PVS:** When vis data is absent (Fast build profile, or corrupted BSP), draw all faces. Slower but correct. Frustum culling still applies.

---

## 3. BSP Loading Pipeline

**Target format:** BSP2 (`qbsp -bsp2`). BSP2 removes BSP29's geometry limits (face counts, vertex counts, coordinate range). The qbsp crate auto-detects format.

Loader parses BSP and BSPX lumps into engine-side structs, uploads vertex and lightmap data to GPU buffers. Renderer consumes handles, never raw BSP data. This boundary is strict: raw BSP types do not appear in renderer code.

**Load sequence:**

1. Parse BSP file via qbsp. This produces typed access to standard lumps (vertices, edges, faces, textures, visibility, lighting) and BSPX extension lumps.
2. Build engine-side vertex data in the custom vertex format (see §6).
3. Pack per-face lightmap samples into a shared atlas texture (CPU-side).
4. Parse optional BSPX lumps (see §4) and attach results to the appropriate engine structures.
5. Build per-face metadata: material type, lightmap atlas region, draw command parameters.
6. Hand prepared data to the renderer. The renderer performs all GPU uploads (vertex buffer, index buffer, lightmap atlas) — the loader never calls wgpu.
7. Renderer returns opaque handles. All subsequent draw operations reference these handles.

---

## 4. BSPX Lump Consumption

BSPX lumps are optional extensions baked into the BSP file by ericw-tools. Each lump enriches rendering fidelity when present and degrades gracefully when absent (see §5).

### RGBLIGHTING

Colored lightmaps. Each texel stores RGB instead of monochrome intensity. Sampled as a second texture layer per face, modulating base texture color.

qbsp parses this lump natively into typed data.

### LIGHTINGDIR

Dominant light direction per lightmap texel. Enables approximate per-pixel specular highlights via Blinn-Phong shading — surfaces appear to respond to their baked light direction rather than looking uniformly flat.

qbsp does **not** parse this lump natively. Access the raw bytes via the unparsed-lump API and implement custom parsing.

**Byte layout:** The lump runs parallel to RGBLIGHTING — same number of samples, same ordering (face by face, row by row within each face's lightmap). Each sample is 3 bytes encoding a unit direction vector: `[x, y, z]` where each byte maps to the range [-1, 1] via `component = (byte / 255.0) * 2.0 - 1.0`. The vector points in the dominant light direction for that texel in world space.

Encoding follows the common Quake engine deluxemap convention. ericw-tools 2.0.0-alpha may use a different normalization or coordinate basis.

### AO (Dirt)

Ambient occlusion is baked directly into lightmap data via worldspawn key `_dirt 1`. This is **not** a separate BSPX lump. AO modulates lightmap samples at bake time — crevices and corners receive less light. No engine-side parsing or special handling required; AO is present in every lightmap sample when the map is compiled with dirt enabled.

### LIGHTGRID_OCTREE

Volumetric light probes for dynamic object lighting: sprites, particles, weapon models. Stores irradiance samples in an octree structure that covers the playable volume. Look up the probe nearest to a dynamic object's position and apply its lighting.

qbsp parses this lump natively into a typed octree structure.

**Caveat:** `-lightgrid` was developed primarily for Quake 2 BSP and is experimental for Q1 BSP2. Data may be absent or malformed for Q1 BSP2 maps. If unreliable, fall back to the degradation path in §5.

---

## 5. Degradation Behavior

Every optional lump has a defined fallback. Missing data is not an error — it is a valid, lower-fidelity rendering path. Loader signals absence via an Option. Renderer selects the appropriate path at draw time.

| Missing lump | Fallback |
|--------------|----------|
| **RGBLIGHTING** | Fall back to monochrome LIGHTING lump (standard BSP lighting). If both absent, use a flat white lightmap — geometry is fully lit with no shadows. |
| **LIGHTINGDIR** | Diffuse-only lighting. Same lightmap sampling, no specular term. Surfaces appear flat-lit. |
| **LIGHTGRID_OCTREE** | Sample nearest lightmap face for dynamic object lighting. If no suitable face, use ambient light plus nearest-light approximation. Sprites and particles still receive plausible illumination. |

Log a warning at load time for each absent optional lump. Do not log per-frame.

---

## 6. Vertex Format

Custom vertex format used for all BSP world geometry.

| Attribute | Content | Purpose |
|-----------|---------|---------|
| Position | 3D world-space coordinate | Geometry placement |
| Base texture UV | Texture-space coordinate | Diffuse texture sampling |
| Lightmap UV | Atlas-space coordinate | Lightmap atlas sampling |
| Vertex color | RGBA per-vertex tint | Dynamic lighting contribution, editor visualization, debug overlays |

Lightmap UVs reference regions within the shared lightmap atlas, not individual textures. Atlas region is determined during BSP load and baked into the vertex buffer.

Vertex color is a runtime-writable channel. Base use: per-vertex dynamic light accumulation (muzzle flash falloff, explosion glow). Also available for editor coloring and debug visualization without changing the vertex layout.

---

## 7. Rendering Stages

Forward rendering pipeline. Each stage runs as a distinct render pass or draw call group within a frame.

### 7.1 BSP World Geometry

Draw visible faces from the PVS-culled draw set (§2). Each face samples its base diffuse texture and its region of the lightmap atlas.

When a normal map is present for the surface (see `resource_management.md` §4), the shader samples per-texel surface normals for fine detail. When LIGHTINGDIR data is also present, the normal map and light direction combine for view-dependent Blinn-Phong specular highlights — surfaces respond to both their baked light direction and their fine surface detail as the camera moves.

Fallback tiers (baked lighting only — dynamic lights in §7.2 always use normal maps when present):
- Normal map + LIGHTINGDIR → full specular with surface detail
- LIGHTINGDIR only → specular from lightmap normals, no fine detail
- Normal map only → no baked directional data; no visible baked lighting effect
- Neither → diffuse-only, flat lightmap modulation

### 7.2 Dynamic Lights

Small number of forward point lights that supplement baked lighting: muzzle flashes, neon glow, explosions, projectile trails. These are not a replacement for baked lightmaps — they handle transient, gameplay-driven illumination only.

Dynamic lights accumulate into vertex color or are evaluated per-fragment for nearby faces. Keep the light count low; the baked lightmaps carry the bulk of the lighting work.

When a surface has a normal map, per-fragment dynamic light evaluation should sample it for accurate light response — a bumpy metal panel reacts to a muzzle flash differently than a flat wall. This only applies to per-fragment evaluation; vertex-color accumulation is too coarse for normal map detail.

### 7.3 Billboard Sprites

Camera-facing textured quads for characters, pickups, and decorative elements. Sprites always face the camera (classic Doom-style billboarding).

Lit by the nearest light probe from the LIGHTGRID_OCTREE when available. When the octree is absent, use the fallback described in §5. Sprite lighting should feel consistent with the surrounding environment — a sprite in a dark room appears dark.

### 7.4 Emissive / Fullbright Surfaces

Surfaces that emit light or ignore lightmap attenuation: neon signs, screens, status indicators, glowing panels. These bypass lightmap modulation entirely — their base texture renders at full brightness regardless of baked or dynamic lighting state.

Identified by the emissive flag on the surface's material type (see `resource_management.md` §3). The material prefix determines whether a surface is emissive — resolved during BSP load when materials are derived from texture names.

### 7.5 Fog Volumes

Per-volume fog defined by `env_fog_volume` brush entities. Each fog entity resolves to a set of BSP leaves at load time. At runtime, the camera's current BSP leaf determines the active fog volume (if any). When a leaf belongs to multiple fog volumes, the smallest volume (fewest leaves) wins — same rule as reverb zones (see `audio.md` §6).

Fog is a per-fragment effect applied during geometry rendering, not a screen-space post-process. Fragment shader checks leaf membership in the active fog volume, blends fog color by distance from camera. Fragments outside any fog volume are unaffected.

| Parameter | Source | Effect |
|-----------|--------|--------|
| `color` | `env_fog_volume` entity | Fog blend color |
| `density` | `env_fog_volume` entity | How quickly fog thickens with distance |
| `falloff` | `env_fog_volume` entity | Attenuation curve (linear, exponential, etc.) |

When the camera is outside all fog volumes, fog is disabled — no per-fragment cost.

### 7.6 Post-Processing

Screen-space effects applied after all geometry is drawn.

| Effect | Description |
|--------|-------------|
| **Bloom** | Bright pixels (emissive surfaces, dynamic lights, specular highlights) bleed into surrounding area. Reinforces neon cyberpunk aesthetic. |
| **CRT / Scanline** | Low priority. Optional retro display effects: scanlines, slight curvature, color fringing. Off by default. |

---

## 8. Data Contracts

### BSP loader produces

| Output | Description |
|--------|-------------|
| Vertex buffer | All BSP face vertices in custom vertex format (§6) |
| Index buffer | Triangle indices for all faces |
| Lightmap atlas texture | Packed lightmap samples for all faces, RGB when RGBLIGHTING present |
| Per-face metadata | Material type, lightmap atlas region, draw command parameters (index offset, index count) |
| Optional BSPX data | Parsed LIGHTINGDIR directions, LIGHTGRID_OCTREE probes — each wrapped in Option |

### Renderer consumes

| Input | Description |
|-------|-------------|
| GPU buffer handles | Vertex buffer, index buffer — opaque handles, not raw data |
| Atlas texture handle | Lightmap atlas bound as a texture for sampling |
| Per-face draw commands | Index offset and count per face, grouped by material for batching |
| Optional lighting data | LIGHTINGDIR texture, light probe octree — presence determines shader path |

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

- PVS lookup (§2) — camera position determines the current BSP leaf
- Frustum culling — view-projection matrix defines the clip volume
- All draw calls — view-projection uniform uploaded once per frame

---

## 10. Non-Goals

- **Deferred rendering** — forward pipeline is sufficient for the target light count and aesthetic. Deferred adds complexity without benefit here.
- **Runtime BSP compilation** — maps are compiled offline by ericw-tools. The engine is a consumer, not a compiler.
- **PBR materials** — baked lightmaps and simple Blinn-Phong specular achieve the retro aesthetic. Metallic/roughness workflows are out of scope.
- **Ray tracing** — baked lighting plus a small number of dynamic lights covers the visual needs.
- **Multiplayer / networking** — single-player engine. Network synchronization is not a rendering concern and is excluded from the project scope entirely.
