# Phase 3: Textured World

> **Status:** draft
> **Depends on:** Phase 1 (BSP loading, wireframe renderer, PVS/frustum culling), Phase 2 (fixed-timestep loop, action-mapped camera).
> **Related:** `context/lib/rendering_pipeline.md` · `context/lib/resource_management.md` · `context/lib/development_guide.md`

---

## Goal

Replace wireframe rendering with textured solid geometry using flat uniform lighting. This proves the visual data path: BSP texture name to PNG on disk, sampled per fragment in a single-texture render pipeline. Material derivation from texture name prefixes establishes the hook for footstep sounds, impact effects, and emissive rendering in later phases. Depth buffer and back-face culling introduced here are permanent. CSG face clipping eliminates z-fighting from overlapping PRL brushes.

**Phase 4 adds lighting.** Phase 3 ships with flat uniform white lighting — no lightmaps, no probes. That work is the Phase 4 decision gate.

---

## Scope

### In scope

- Vertex format: position (vec3), base_uv (vec2), vertex_color (vec4). No lightmap UVs.
- Base texture UV computation via `PlanarTextureProjection::project()`, normalized by texture dimensions
- PNG texture loading via `image` crate, matched by BSP texture name strings
- Checkerboard placeholder for missing textures
- Material derivation from texture name prefix (first `_`-delimited token)
- Depth buffer (Depth24Plus or Depth32Float)
- Back-face culling (front = counter-clockwise after coordinate transform)
- Single-texture render pipeline: vertex shader (MVP transform), fragment shader (base texture * flat uniform lighting)
- Per-texture bind groups, draw calls grouped by texture
- CSG face clipping in the PRL compiler to remove faces inside solid brush space (eliminates z-fighting)
- BSP path only for textured rendering (see PRL gap note below)

### Out of scope

- Lightmap atlas (deferred to Phase 5 if Phase 4 probe lighting falls short)
- Lightmap UVs in vertex format (no lightmap_uv attribute)
- BSPX LIGHTING, RGBLIGHTING, DECOUPLED_LM parsing (Phase 4+)
- Light probes / LIGHTGRID_OCTREE (Phase 4)
- World texture atlas batching (optimization, unscheduled)
- Normal maps (Phase 5+)
- Animated lightmap styles (Phase 5+)
- Emissive rendering behavior (material flag is set; bypass is Phase 5)
- Dynamic lights (Phase 5+)
- Fog volumes (Phase 5+)
- Billboard sprites (Phase 5+)
- Texture hot-reload

### PRL gap

The PRL compiler parses texture names from `.map` files but discards them at pack time. The PRL format has no texture data section. Phase 3 textured rendering targets BSP exclusively. PRL levels fall back to wireframe or solid-unlit rendering. Texture data in PRL is a future task, unscheduled.

CSG face clipping (Task 05) is PRL-only — it runs in the compiler to fix z-fighting in PRL geometry. BSP already handles this via BSP tree construction.

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
| vertex_color | vec4 f32 | 16 bytes | RGBA per-vertex tint (white default; Phase 5 uses for dynamic lights) |

Total stride: 36 bytes. Vertex buffer layout must match exactly across loader and shader.

No lightmap_uv attribute. Phase 4 evaluates probe-based lighting; lightmap UVs are only needed if Phase 5 falls back to lightmap atlas.

### Flat uniform lighting

Phase 3 fragment shader applies a flat ambient factor rather than sampled lightmap data. A uniform `vec3` (e.g., `[1.0, 1.0, 1.0]`) multiplied with the base texture sample. This is intentionally simple — Phase 4 replaces it with probe-sampled lighting.

### Texture name matching

BSP stores texture names in `BspMipTexture.header.name`. Engine searches `textures/` recursively for `<name>.png` (case-insensitive match on filename stem). The collection subdirectory is not stored in BSP — search all collections.

### Missing data degradation

| Condition | Behavior |
|-----------|----------|
| Missing texture PNG | Checkerboard placeholder (magenta/black, 64x64). Log warning per missing texture. |
| Unknown material prefix | Default material. Log warning per unique unknown prefix. |
| PRL level loaded | Wireframe or solid-unlit fallback. No texture data available in PRL format. |

---

## Task List

| ID | Task | File | Dependencies | Description |
|----|------|------|-------------|-------------|
| 01 | Vertex Format and Base Texture UVs | `task-01-vertex-format.md` | none | Replace wireframe vertex format with textured format (no lightmap_uv). Compute base texture UVs. Extend face metadata. Update LevelGeometry. |
| 02 | Texture Loading | `task-02-texture-loading.md` | none | Load PNGs matched by BSP texture names. Checkerboard fallback. CPU-side RGBA8 output to renderer. |
| 03 | Material Derivation | `task-03-material-derivation.md` | none | Parse texture name prefix, map to material enum, attach to per-face metadata. |
| 04 | Render Pipeline | `task-04-render-pipeline.md` | 01, 02 | Replace wireframe with solid single-texture pipeline. Depth buffer, back-face culling, flat uniform lighting. |
| 05 | CSG Face Clipping | `task-05-csg-face-clipping.md` | none | Clip PRL faces against overlapping brush volumes at compile time. Removes faces inside solid space, eliminates z-fighting. |

---

## Execution Order

### Dependency graph

```
  +-------------------+     +------------------+     +------------------+     +------------------+
  | 01 Vertex format  |     | 02 Texture        |     | 03 Material       |     | 05 CSG face      |
  | and base UVs      |     | loading           |     | derivation        |     | clipping         |
  +--------+----------+     +--------+----------+     +------------------+     +------------------+
           |                         |
           +-------------------------+
                       |
              +--------v---------+
              | 04 Render        |
              | pipeline         |
              +------------------+
```

### Concurrency rules

| Wave | Tasks | Notes |
|------|-------|-------|
| Wave 1 (parallel) | 01, 02, 03, 05 | No dependencies. Start all in parallel. Task 01 is on the critical path to 04. |
| Wave 2 | 04 | Depends on 01 and 02. Run after both complete. Task 03 output (material metadata) does not block 04 — ship for data completeness. |

### Orchestrator notes

- Task 01 requires a test map with PNG textures under `textures/` for verification. The BSP does not need recompilation for Phase 3 — flat uniform lighting requires no baked data. Create simple solid-color 64x64 PNGs if none exist.
- Tasks 02, 03, and 05 produce CPU-side data only. None touch wgpu. All can run in parallel with Task 01.
- Task 04 is the integration point. Consumes vertex buffers (01) and loaded textures (02). Removes Phase 1 wireframe infrastructure. The implementing agent needs familiarity with the renderer module from Phase 1.
- Task 03 has no dependents within Phase 3. Lowest-risk task. Material data is consumed by Phase 4+ systems.
- Task 05 runs in the PRL compiler, not the engine. Independent of the BSP rendering tasks. Can run in any wave.

---

## Acceptance Criteria

1. **Textured rendering works.** `cargo run -- assets/maps/test.bsp` shows textured BSP geometry with uniform flat lighting. No wireframe.
2. **Uniform lighting.** All surfaces lit at the same constant level. No per-surface variation, no shadows. (Phase 4 adds probe lighting.)
3. **Missing textures degrade.** Remove a PNG — affected surfaces render with checkerboard. Warning logged. No crash.
4. **Depth buffer works.** No z-fighting. Near geometry occludes far geometry correctly.
5. **Back-face culling works.** Interior room faces visible; exterior back-faces culled.
6. **Materials are derived.** `RUST_LOG=info` shows material assignments. Unknown prefixes produce warnings.
7. **Module boundary holds.** BSP loader and texture loader contain zero wgpu imports. Renderer does not parse BSP structures or PNG files.
8. **CSG clipping compiles.** Compiling a `.map` with overlapping brushes to PRL produces geometry without z-fighting in the output.
9. **PRL levels still load.** PRL levels render in wireframe or solid-unlit. No crash, no missing texture errors (expected — PRL has no texture data).

---

## What Carries Forward

| Output | Consumed by |
|--------|-------------|
| Vertex format (position, base UV, vertex color) | Phase 4 extends with probe-sampled lighting via vertex_color or a new per-frame uniform. |
| Texture loading pipeline | Every phase that adds visual assets. |
| Material enum and prefix derivation | Phase 4 (footsteps), Phase 5 (emissive bypass), Phase 7 (surface interaction). |
| Depth buffer | Every phase — solid rendering requires depth testing. |
| Flat uniform lighting pipeline | Phase 4 replaces uniform factor with probe-sampled value. |
| CSG face clipping | Permanent compiler step for PRL geometry. |

### Replaced in later phases

| Phase 3 artifact | Replaced by | When |
|------------------|-------------|------|
| Flat uniform lighting | Probe-sampled per-surface lighting | Phase 4 |
| Per-texture individual bind | World texture atlas (batched draw calls) | Optimization pass (unscheduled) |
| Material derivation (stub behaviors) | Material behaviors with real consumers | Phase 4 (footsteps), Phase 5 (emissive) |
