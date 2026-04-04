# Phase 3: Textured World

> **Status:** draft
> **Depends on:** Phase 1 (BSP loading, wireframe renderer, PVS/frustum culling), Phase 2 (fixed-timestep loop, action-mapped camera).
> **Related:** `context/lib/rendering_pipeline.md` §3, §6, §7.1 · `context/lib/resource_management.md` §1–3 · `context/lib/build_pipeline.md` · `context/lib/development_guide.md`

---

## Goal

Replace wireframe rendering with textured, lit solid geometry. This proves the full visual data path: BSP texture name to PNG on disk, BSP lightmap data to GPU atlas, both sampled per fragment in a two-texture render pipeline. Material derivation from texture name prefixes establishes the hook point for footstep sounds, impact effects, and emissive rendering in later phases. The depth buffer and back-face culling introduced here are permanent — every subsequent rendering phase inherits them.

---

## Scope

### In scope

- Full vertex format: position (vec3), base_uv (vec2), lightmap_uv (vec2), vertex_color (vec4)
- Base texture UV computation via `PlanarTextureProjection::project()`, normalized by texture dimensions
- Lightmap atlas construction: per-face lightmap extraction, shelf-packing, atlas-space UV computation
- Monochrome LIGHTING lump parsing (grayscale lightmaps)
- BSPX RGBLIGHTING: upgrade atlas to RGB when present
- BSPX DECOUPLED_LM: per-face independent lightmap projection and dimensions when present
- PNG texture loading via `image` crate, matched by BSP texture name strings
- Checkerboard placeholder for missing textures
- Material derivation from texture name prefix (first `_`-delimited token)
- Depth buffer (Depth24Plus or Depth32Float)
- Back-face culling (front = counter-clockwise after coordinate transform)
- Textured render pipeline: vertex shader (MVP transform), fragment shader (base texture * lightmap)
- Per-texture bind group, draw calls grouped by texture
- Lightmap style 0 (static) only

### Out of scope

- World texture atlas (draw-call batching optimization — individual texture binds for now)
- Normal maps (Phase 5, interacts with LIGHTINGDIR)
- LIGHTINGDIR (Phase 5, per-pixel specular)
- LIGHTGRID_OCTREE (Phase 5, sprite lighting)
- Animated lightmap styles (styles 1–3 deferred)
- Emissive rendering behavior (material flag is set; rendering bypass is Phase 5)
- Dynamic lights (Phase 5)
- Fog volumes (Phase 5)
- Billboard sprites (Phase 5)
- Texture hot-reload

---

## Shared Context

Cross-task decisions. Each task file restates its relevant subset — this is the single source.

### Coordinate transform

Quake BSP uses right-handed Z-up. Engine uses Y-up (glam default). Phase 1 applies: swap Y and Z, negate the new Z. UV computation operates on pre-transform BSP vertices (Z-up) because `PlanarTextureProjection::project()` expects Quake-space coordinates. Apply the coordinate transform after UV computation, or pass untransformed positions to the projection and transform separately.

### Vertex format

| Attribute | Type | Size | Content |
|-----------|------|------|---------|
| position | vec3 f32 | 12 bytes | World-space coordinate (Y-up, post-transform) |
| base_uv | vec2 f32 | 8 bytes | Texture-space UV, normalized by texture dimensions |
| lightmap_uv | vec2 f32 | 8 bytes | Atlas-space UV referencing the face's lightmap region |
| vertex_color | vec4 f32 | 16 bytes | RGBA per-vertex tint (white default, dynamic lights in Phase 5) |

Total stride: 44 bytes. Vertex buffer layout must match exactly across loader and shader.

### Lightmap dimension computation

Two paths depending on BSPX lump presence:

**Standard path (no DECOUPLED_LM):** For each face, project all vertices via `BspTexInfo.projection`, find min/max in both axes, compute dimensions as `floor((max - min) / 16) + 1`. Quake standard: one lightmap sample per 16 texels.

**DECOUPLED_LM path:** Read `DecoupledLightmap.size` directly. Dimensions and projection are per-face and independent of the texture info. Use `DecoupledLightmap.projection` for lightmap UV computation instead of the texture info projection.

### Lightmap UV computation

1. **Face-local lightmap space:** Project vertex position via the appropriate projection (standard texinfo or DECOUPLED_LM). Subtract face minimum, divide by 16 (standard) or by lightmap dimensions (decoupled). This gives [0, lightmap_width) and [0, lightmap_height) range.
2. **Atlas space:** Scale and offset by the face's position within the packed atlas. Add 0.5-texel offset to center sampling within each lightmap texel.

### Texture name matching

BSP stores texture names in `BspMipTexture.header.name`. Engine searches `textures/` recursively for `<name>.png` (case-insensitive match on filename stem). The collection subdirectory is not stored in BSP — search all collections.

### Missing data degradation

| Condition | Behavior |
|-----------|----------|
| Missing RGBLIGHTING | Use monochrome LIGHTING lump. Atlas stores intensity as RGB. |
| Missing LIGHTING entirely | Flat white lightmap (all 255). Geometry fully lit, no shadows. Log warning once. |
| Missing DECOUPLED_LM | Standard texinfo-based lightmap dimension computation. |
| Missing texture PNG | Checkerboard placeholder (magenta/black, 64x64). Log warning per missing texture. |
| Unknown material prefix | Default material. Log warning per unique unknown prefix. |

### Test map bootstrap

Phase 1's test map was compiled without lighting. Phase 3 needs lightmap data and texture references.

Recompile: `light -bspx assets/maps/test.bsp`

This adds LIGHTING and RGBLIGHTING lumps. DECOUPLED_LM may or may not be present depending on ericw-tools defaults — handle both paths. PNG textures must exist under `textures/` for every texture name referenced by the test map.

---

## Task List

| ID | Task | File | Dependencies | Description |
|----|------|------|-------------|-------------|
| 01 | Full Vertex Format and Base Texture UVs | `task-01-vertex-format.md` | none | Upgrade vertex from position-only to full format. Compute base texture UVs. Bootstrap: recompile test map with lighting. |
| 02 | Lightmap Atlas Construction | `task-02-lightmap-atlas.md` | 01 | Compute per-face lightmap dimensions, extract samples, shelf-pack into atlas, compute atlas-space UVs. |
| 03 | Texture Loading | `task-03-texture-loading.md` | none | Load PNGs matched by BSP texture names. Checkerboard fallback. Hand CPU-side RGBA8 data to renderer. |
| 04 | Material Derivation | `task-04-material-derivation.md` | none | Parse texture name prefix, map to material enum, attach to per-face metadata. |
| 05 | Textured Render Pipeline | `task-05-render-pipeline.md` | 01, 02, 03 | Replace wireframe with solid textured pipeline. Depth buffer, back-face culling, two-texture sampling. |

---

## Execution Order

### Dependency graph

```
        +-------------------+
        | 01 Vertex format  |
        +--+----------+-----+
           |          |
    +------v---+      |      +--------------+     +------------------+
    | 02 Light-|      |      | 03 Texture   |     | 04 Material      |
    | map atlas|      |      | loading      |     | derivation       |
    +------+---+      |      +------+-------+     +------------------+
           |          |             |
           +----------+-------------+
                      |
               +------v------+
               | 05 Render   |
               | pipeline    |
               +-----------  +
```

### Concurrency rules

| Wave | Tasks | Notes |
|------|-------|-------|
| Wave 1 (parallel) | 01, 03, 04 | Task 01 is the critical path (02 and 05 depend on it). Tasks 03 and 04 have no dependencies — start in parallel with 01. |
| Wave 2 | 02 | Depends on 01 (needs full vertex format and face metadata with texture dimensions). Can overlap with 03/04 if they aren't done yet. |
| Wave 3 | 05 | Consumes outputs from 01, 02, and 03. Task 04 output (material metadata) is not consumed by the render pipeline until Phase 5, but should complete in the same phase for data completeness. |

### Notes for the orchestrator

- Task 01 includes a test map recompile bootstrap step. The existing `assets/maps/test.bsp` must be recompiled with `light -bspx` before lightmap work can proceed. Task 01 also requires PNG textures to exist under `textures/`.
- Tasks 03 and 04 produce CPU-side data only. Neither touches wgpu. They can run fully in parallel with 01 and 02.
- Task 05 is the integration point. It consumes vertex buffers (01), lightmap atlas (02), and loaded textures (03). The implementing agent needs familiarity with the renderer module established in Phase 1.
- Task 04 is the lowest-risk task and has no dependents within this phase.

---

## Acceptance Criteria

1. **Textured rendering works.** `cargo run -- assets/maps/test.bsp` shows textured, lit BSP geometry. No wireframe, no blank surfaces.
2. **Lightmaps apply correctly.** Surfaces in shadow are visibly darker. No lightmap seams between adjacent faces. No UV misalignment.
3. **Colored lightmaps work.** RGBLIGHTING present: colored light tints surfaces. Absent: monochrome lighting, no error.
4. **Missing textures degrade.** Remove a PNG — affected surfaces render with checkerboard. Warning logged. No crash.
5. **Missing lightmaps degrade.** Load a BSP compiled without `light`. All surfaces fully lit. Warning logged once. No crash.
6. **Depth buffer works.** No z-fighting. Near geometry occludes far geometry correctly.
7. **Back-face culling works.** Interior room faces visible; exterior back-faces culled. Face count lower than wireframe.
8. **Materials are derived.** `RUST_LOG=info` shows material assignments. Unknown prefixes produce warnings.
9. **DECOUPLED_LM handled.** If present: lightmaps render correctly using decoupled dimensions. If absent: standard path works.
10. **Module boundary holds.** BSP loader and texture loader contain zero wgpu imports. Renderer does not parse BSP structures or PNG files.

---

## What Carries Forward

### Durable outputs

| Output | Consumed by |
|--------|-------------|
| Full vertex format (position, base UV, lightmap UV, vertex color) | Every rendering phase. Vertex color used by Phase 5 dynamic lights. |
| Lightmap atlas construction | Phase 5 extends with LIGHTINGDIR sampling. |
| Texture loading pipeline | Every phase that adds visual assets. |
| Material enum and prefix derivation | Phase 4 (footsteps), Phase 5 (emissive bypass), Phase 7 (surface interaction). |
| Depth buffer | Every phase — solid rendering requires depth testing. |
| Textured render pipeline | Phase 5 extends with specular, emissive, dynamic lights. |
| DECOUPLED_LM handling | Permanent — lightmap computation always checks for decoupled data. |

### Replaced in later phases

| Phase 3 artifact | Replaced by | When |
|------------------|-------------|------|
| Per-texture individual bind | World texture atlas (batched draw calls) | Optimization pass (unscheduled) |
| Lightmap atlas (diffuse only) | Lightmap atlas + directional lightmap atlas | Phase 5 (LIGHTINGDIR) |
| Material derivation (stub behaviors) | Material behaviors with real consumers | Phase 4 (footsteps), Phase 5 (emissive) |
