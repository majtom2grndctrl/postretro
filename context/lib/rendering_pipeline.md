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
| **Render** | Determine visible set (BSP leaves or PRL clusters), draw visible geometry, dynamic lights, sprites, post-processing |
| **Present** | Swap buffers |

Game logic runs at a fixed timestep decoupled from render rate. Renderer interpolates between the last two game states for smooth visuals at variable framerates. Simulation is deterministic at any refresh rate. Rendering never blocks or drives the simulation clock.

**Edge cases:** On the first frame, only one game state exists — duplicate it so interpolation produces the initial state with no blending. After a long stall (alt-tab, disk I/O), clamp the accumulator (e.g., 250ms max) to prevent dozens of catch-up ticks.

---

## 2. Visibility and Traversal

Visibility is determined per-frame using precomputed Potentially Visible Set (PVS) data. Two formats provide PVS differently, but the rendering result is the same: a set of face ranges to draw.

### BSP path (current)

1. Determine which BSP leaf contains the camera position (BSP tree traversal).
2. Look up the PVS for that leaf — a compressed bitfield of which other leaves are potentially visible.
3. Decompress the PVS into a visible leaf set.
4. Collect all faces belonging to visible leaves.
5. Submit collected faces as the frame's draw set.

### PRL path

1. Determine which cluster contains the camera position (bounding volume scan; clusters have expanded bounds from air cell merging to ensure full coverage).
2. Look up the PVS for that cluster — a compressed bitfield of which other clusters are potentially visible.
3. Frustum-cull visible clusters by bounding volume.
4. Collect face ranges for surviving clusters.
5. Submit collected faces as the frame's draw set.

PVS data is precomputed by the level compiler using voxel-based ray-casting (3D-DDA through a solid/empty bitmap). See `build_pipeline.md` §PRL for compiler details.

PVS culling is conservative in both paths: it may include faces that are technically occluded, but never excludes a visible face. Slight overdraw is cheaper than per-face occlusion tests.

Frustum culling further reduces the draw set by discarding faces/clusters outside the camera's view volume. PVS runs first (coarse), frustum culling runs second (fine).

**Missing PVS:** When vis data is absent (Fast build profile, corrupted BSP, or PRL without a visibility section), draw all faces. Slower but correct. Frustum culling still applies.

---

## 3. BSP Loading Pipeline

**Target format:** BSP2 (`qbsp -bsp2`). BSP2 removes BSP29's geometry limits (face counts, vertex counts, coordinate range). The qbsp crate auto-detects format.

Loader parses BSP lumps into engine-side structs, produces GPU-ready data. Renderer consumes handles, never raw BSP data. This boundary is strict: raw BSP types do not appear in renderer code.

**Load sequence:**

1. Parse BSP file via qbsp. Typed access to vertices, edges, faces, textures, and visibility data.
2. Build engine-side vertex data: positions (coordinate-transformed to engine Y-up), texture UVs (computed from BSP face projection data), vertex color (white default). See §6.
3. Load PNG textures matched by BSP texture name strings. Generate checkerboard placeholders for missing textures.
4. Build per-face metadata: material type (from texture name prefix), texture index, draw command parameters.
5. Sort faces by (leaf, texture) for draw batching. Pre-compute per-leaf texture sub-ranges.
6. Hand prepared data to the renderer. Renderer performs all GPU uploads — loader never calls wgpu.
7. Renderer returns opaque handles. All subsequent draw operations reference these handles.

---

## 4. BSPX Lump Consumption

> **Phase 4+. Not yet implemented.** BSPX parsing is planned for the lighting phase. The section below describes intended behavior.

BSPX lumps are optional extensions baked into the BSP file by ericw-tools. Each lump enriches rendering fidelity when present and degrades gracefully when absent (see §5).

### RGBLIGHTING

Colored lightmaps. Each texel stores RGB instead of monochrome intensity. Sampled as a texture layer per face, modulating base texture color.

qbsp parses this lump natively into typed data.

### LIGHTINGDIR

Dominant light direction per lightmap texel. Enables approximate per-pixel specular highlights — surfaces respond to their baked light direction rather than looking uniformly flat.

qbsp does **not** parse this lump natively. Access raw bytes via the unparsed-lump API.

**Byte layout:** Runs parallel to RGBLIGHTING — same sample count, same ordering. Each sample is 3 bytes encoding a unit direction in world space: each byte maps to [-1, 1] via `(byte / 255.0) * 2.0 - 1.0`. Follows the common Quake deluxemap convention. ericw-tools 2.0.0-alpha may use a different normalization — verify early.

### AO (Dirt)

Ambient occlusion baked directly into lightmap data via worldspawn key `_dirt 1`. Not a separate lump. No engine-side parsing required — AO is present in every lightmap sample when compiled with dirt enabled.

### DECOUPLED_LM

Per-face independent lightmap projection and dimensions. Breaks the standard coupling between texture UV scale and lightmap resolution.

qbsp parses this lump natively (per-face size, offset, projection). When present, use its dimensions and projection for UV computation instead of the texinfo-derived values.

### LIGHTGRID_OCTREE

Volumetric light probes for dynamic object lighting: sprites, particles, weapon models. Irradiance samples in an octree covering the playable volume.

qbsp parses this lump natively. **Caveat:** `-lightgrid` was developed for Quake 2 BSP and is experimental for Q1 BSP2. Verify probe data is usable early; fall back to degradation path (§5) if not.

---

## 5. Degradation Behavior

> **Phase 4+.** Degradation paths for BSPX lumps become relevant when Phase 4 implements lump parsing. Current behavior: flat white ambient lighting regardless of what lumps are present.

Every optional lump has a defined fallback. Missing data is not an error — it is a valid, lower-fidelity path. Loader signals absence via `Option`. Renderer selects the appropriate path at draw time.

| Missing lump | Fallback |
|--------------|----------|
| **RGBLIGHTING** | Monochrome LIGHTING lump. If both absent: flat white lightmap — fully lit, no shadows. |
| **LIGHTINGDIR** | Diffuse-only. Same lightmap sampling, no specular term. |
| **DECOUPLED_LM** | Standard texinfo-based lightmap UV computation. |
| **LIGHTGRID_OCTREE** | Sample nearest lightmap face for dynamic object lighting. If none suitable: ambient plus nearest-light approximation. |

Log a warning at load time per absent optional lump. Do not log per-frame.

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

Draw visible faces from the PVS-culled draw set (§2). Draw calls grouped by (leaf, texture) — one call per visible leaf × texture pair. Minimizes bind group switches without breaking leaf contiguity required by PVS.

Each face samples its base texture at its UV coordinate. Flat ambient lighting applied uniformly: `output = base_texture × ambient_light × vertex_color`. Phase 4 replaces the flat ambient factor with probe-sampled per-surface values.

Depth testing (Less, write enabled) and back-face culling (counter-clockwise front face) are permanent from this phase forward.

### 7.2 Dynamic Lights

> **Phase 5+. Not yet implemented.**

Forward point lights supplementing baked lighting: muzzle flashes, neon glow, explosions, projectile trails. Transient, gameplay-driven illumination — not a replacement for baked lighting. Accumulate into vertex color or evaluate per-fragment.

### 7.3 Billboard Sprites

> **Phase 5+. Not yet implemented.**

Camera-facing textured quads for characters, pickups, and decorative elements. Classic Doom-style billboarding. Lit by nearest LIGHTGRID_OCTREE probe; falls back to §5 when absent.

### 7.4 Emissive / Fullbright Surfaces

> **Rendering behavior Phase 5+.** Material flag is derived and stored during BSP load. The rendering bypass is not yet implemented.

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
| Per-leaf texture sub-ranges | Drive the draw loop: one draw_indexed() per (visible_leaf, texture) pair |

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
- **Runtime level compilation** — maps are compiled offline (ericw-tools for BSP, prl-build for PRL). The engine is a consumer, not a compiler.
- **PBR materials** — baked lightmaps and simple Blinn-Phong specular achieve the retro aesthetic. Metallic/roughness workflows are out of scope.
- **Ray tracing** — baked lighting plus a small number of dynamic lights covers the visual needs.
- **Multiplayer / networking** — single-player engine. Network synchronization is not a rendering concern and is excluded from the project scope entirely.
