# Build Pipeline

> **Read this when:** setting up the map authoring toolchain, modifying the asset pipeline, adding custom entities, or debugging map compilation issues.
> **Key invariant:** maps are authored in TrenchBroom. Engine canonical unit: 1 unit = 1 meter. PRL is the sole runtime map format.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md)

---

## Pipeline Overview

Maps are authored in TrenchBroom, compiled to PRL with prl-build:

```
TrenchBroom (.map) ──► prl-build (postretro-level-compiler) ──► PRL file (.prl)

Engine loads PRL + PNGs at runtime
```

prl-build builds a BSP tree as a compiler intermediate, generates portal geometry, builds a global BVH over all static triangles, and packs runtime data into a custom binary format. BSP drives spatial partitioning and portal generation at compile time; the runtime consumes cells, portals, and BVH arrays. Engine loads via the `postretro-level-format` crate.

---

## Supported Map Formats

prl-build accepts idTech2 `.map` files (Quake 1/2 dialect, parsed via shambler/shalrath). Unit scale: 1 unit = 0.0254 m (one inch, exact).

Both Standard (axis-aligned) and Valve 220 (explicit UV axes) texture projections are supported. Shalrath auto-detects per face; they can coexist in one `.map` file.

---

## PNG Texture Pipeline

No WAD files. Textures are authored as PNGs.

| Stage | What happens |
|-------|-------------|
| Author | Create PNGs in `textures/<collection>/<name>.png`. TrenchBroom requires one subdirectory level. |
| TrenchBroom | Browses the textures directory via the Postretro game config. |
| prl-build | Reads PNGs for dimensions during compilation. |
| PRL output | TextureNames section stores a deduplicated texture name list. No pixel data. |
| Engine | Loads PNGs at runtime, matched to PRL texture entries by name string. |

---

## Custom FGD

Project deliverable alongside the engine. Defines Postretro-specific entities for TrenchBroom.

| Entity | Type | Purpose | Key Properties |
|--------|------|---------|----------------|
| `light` | point | Omnidirectional light | `light` (intensity), `_color` (RGB), `_fade` (falloff distance, required), `delay` (falloff model), `style` (animation), `_phase` (style cycle offset) |
| `light_spot` | point | Spotlight with cone | + `_cone`, `_cone2` (inner/outer angles), `mangle` (direction) |
| `light_sun` | point | Directional sun light | + `mangle` (direction vector) |
| `env_fog_volume` | brush | Per-region fog | `color`, `density`, `falloff` |
| `env_cubemap` | point | Reflection probe position | `size` (resolution per face; default 256) |
| `env_reverb_zone` | brush | Acoustic zone | `reverb_type`, `decay_time`, `occlusion_factor` |

### Entity resolution

- **`light`, `light_spot`, `light_sun`** — validated at compile time (falloff distance required, spotlight direction verified, intensity bounds checked). Feed the SH irradiance volume baker and the runtime direct lighting path. Compilation fails on validation errors.
- **`env_fog_volume`** — resolved to BSP leaves at load time. Each leaf in the volume gets per-leaf fog parameters.
- **`env_cubemap`** — marks a position for offline cubemap baking. Bake tool is out of initial scope.
- **`env_reverb_zone`** — resolved to BSP leaves at load time. Each leaf gets spatial reverb parameters for the audio subsystem.

---

## Surface Material Derivation

Texture name prefix maps to a material enum. Drives footstep sounds, bullet impacts, and decals. The engine provides the prefix-to-material lookup mechanism; which prefixes exist is a game content concern. The table grows as content requires it.

Example: `metal_floor_01` → Metal, `concrete_wall_03` → Concrete. See `resource_management.md` §3 for the full mechanism and behavior hooks.

Unknown prefix falls back to a default material with a warning at load time.

---

## PRL Compilation

### Compiler pipeline

```
parse .map → BSP construction → brush-side projection → portal generation → exterior leaf culling → portal vis → geometry → BVH → pack .prl
```

1. **Parse.** Extracts brush volumes, brush sides, and entities. Applies coordinate transform (Quake Z-up → engine Y-up) and unit scale. Light entities route to FGD translation and validation; they don't participate in BSP construction.
2. **BSP construction.** Partitions world space into solid and empty leaves using brush-derived planes. Leaf solidity is established during construction from the brush half-space intersection — not inferred from face positions afterward.
3. **Brush-side projection.** Derives visible world faces from brush sides. Produces triangulated geometry per empty leaf; faces in solid space are discarded.
4. **Portal generation.** Clips splitting-plane polygons against ancestor planes to produce convex portals connecting adjacent empty leaves. Stored in PRL for runtime traversal (default) or consumed by vis (`--pvs` mode) and discarded.
5. **Exterior leaf culling.** Flood-fills through the portal graph from outside the map boundary. Exterior-reachable leaves produce no geometry. A map with a leak has interior leaves incorrectly classified as exterior.
6. **Portal vis** (`--pvs` mode only). Computes per-leaf PVS bitsets by flooding through the portal graph. Output: RLE-compressed bitsets.
7. **Geometry.** Fan-triangulates faces into a global vertex/index buffer. Associates each face with a material bucket and cell ID.
8. **BVH.** Builds a global SAH BVH over all static geometry organized by `(face, material_bucket)` pair. Flattens to dense arrays; leaves sorted by material bucket for contiguous per-bucket indirect draw slots.
9. **Pack.** Writes all sections to the `.prl` binary format.

### PRL section IDs

| Section | ID | When present |
|---------|-----|-------------|
| BspNodes | 12 | Always |
| BspLeaves | 13 | Always |
| LeafPvs | 14 | `--pvs` mode only |
| Portals | 15 | Default mode |
| TextureNames | 16 | Always |
| Geometry | 17 | Always |
| AlphaLights | 18 | Always |
| Bvh | 19 | Always |
| ShVolume | 20 | When compiled with lighting |
| LightInfluence | 21 | When compiled with lighting |

### Runtime visibility

Two paths, selected by which PRL section is present:

| PRL section present | Runtime path |
|---------------------|--------------|
| Portals (15) | Per-frame portal flood-fill with frustum narrowing |
| LeafPvs (14) | Precomputed PVS bitset lookup |

Portal traversal is the default and preferred path. See `rendering_pipeline.md` §2.

---

## Non-Goals

- Runtime level compilation
- WAD file support
- Runtime lightmap baking
