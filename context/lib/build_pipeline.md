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

> **Format adapter boundary:** PRL is the engine's internal coordinate standard; Quake convention is not. The `format/` layer in the level compiler is the adapter boundary — each input format translates its own coordinate axes, angle encoding, and units to engine convention before reaching shared compiler logic. Format-specific helpers belong in the format adapter, not shared code.

---

## PNG Texture Pipeline

No WAD files. Textures are authored as PNGs.

| Stage | What happens |
|-------|-------------|
| Author | Create PNGs in `content/<mod>/textures/<collection>/<name>.png` (where `<mod>` is `base` for first-party content or `tests` for fixtures). TrenchBroom requires one subdirectory level. |
| TrenchBroom | Browses the textures directory via the Postretro game config. |
| prl-build | Reads PNGs for dimensions during compilation. |
| PRL output | TextureNames section stores a deduplicated texture name list. No pixel data. |
| Engine | Loads PNGs at runtime, matched to PRL texture entries by name string. |

---

## Custom FGD

Project deliverable alongside the engine. Defines Postretro-specific entities for TrenchBroom.

| Entity | Type | Purpose | Key Properties |
|--------|------|---------|----------------|
| `light` | point | Omnidirectional light | `light` (intensity), `_color` (RGB), `_fade` (falloff distance, required), `delay` (falloff model), `style` (animation), `_phase` (style cycle offset), `_dynamic` (static-baked vs. runtime dynamic; default 0 = static) |
| `light_spot` | point | Spotlight with cone | + `_cone`, `_cone2` (inner/outer angles), `angles` (direction) |
| `light_sun` | point | Directional sun light | + `angles` (direction vector) |
| `env_fog_volume` | brush | Per-region fog | `color`, `density`, `falloff`, `scatter` (scatter fraction toward camera; default 0.6) |
| `billboard_emitter` | point | Billboard particle emitter | `rate` (particles/sec; default 6), `lifetime` (seconds; default 3), `spread` (cone half-angle radians; default 0.4), `buoyancy` (-1=falls, 0=floats, >0=rises; default 0.2), `drag` (velocity damping/sec; default 0.8), `sprite` (collection name; default "smoke"), `initial_velocity_x/y/z` (default 0/0.8/0), `color_r/g/b` (linear; default 1/1/1), `spin_rate` (radians/sec; default 0) |
| `env_cubemap` | point | Reflection probe position | `size` (resolution per face; default 256) |
| `env_reverb_zone` | brush | Acoustic zone | `reverb_type`, `decay_time`, `occlusion_factor` |
| `worldspawn` | special | Scene-wide render settings | `script` (path to entry `.ts` script, relative to `.map` file; compiled by `prl-build`), `data_script` (path to data script file; TS compiled to JS via scripts-build, Luau passed through; absent = no data script), `ambient_color` (RGB ambient floor), `fog_pixel_scale` (volumetric pass resolution divisor; default 4, range 1–8) |

### Entity resolution

- **`light`, `light_spot`, `light_sun`** — validated at compile time (falloff distance required, spotlight direction verified, intensity bounds checked). Static lights feed the SH irradiance volume baker and the directional lightmap baker. Dynamic lights feed the runtime direct lighting buffer. Compilation fails on validation errors.
- **`env_fog_volume`** — resolved at load time to world-space AABBs and fog parameters. Uploaded as a compact storage buffer (up to 16 entries). Runtime uses point-in-AABB membership test per raymarch sample; no BSP traversal at runtime.
- **`billboard_emitter`** — resolved at level load via the built-in classname dispatch table. The engine spawns an ECS entity with a `BillboardEmitterComponent` configured from the map's KVPs. See §Built-in classname routing below.
- **`env_cubemap`** — marks a position for offline cubemap baking. Bake tool is out of initial scope.
- **`env_reverb_zone`** — resolved to BSP leaves at load time. Each leaf gets spatial reverb parameters for the audio subsystem.

---

## Built-in Classname Routing

The level loader resolves FGD `classname` values against an engine-side handler table (`ClassnameDispatch`) at level load. This table is populated once at engine init by `register_builtins()` and is never cleared on level unload — built-in handlers describe engine types, not per-level state.

For each map entity:
1. Look up `entity.classname` in the built-in handler table.
2. If found: instantiate the configured components, apply the KVP map, spawn the ECS entity at `entity.origin`, copy `_tags`.
3. If not found: `log::debug!` and skip — unregistered classnames are valid in maps that don't use them.

Invalid KVP values log a warning naming the key and entity origin, fall back to the documented default, and load continues.

**Current built-in types:** `billboard_emitter`.

**Production status:** the PRL wire format does not yet carry a generic map-entity section. The `ClassnameDispatch` table and handlers are live and ready; once the section ships and `LevelWorld.map_entities` is populated at PRL load, the dispatch fires automatically with no further engine changes.

---

## Surface Material Derivation

Texture name prefix maps to a material enum. Drives footstep sounds, bullet impacts, and decals. The engine provides the prefix-to-material lookup mechanism; which prefixes exist is a game content concern. The table grows as content requires it.

Example: `metal_floor_01` → Metal, `concrete_wall_03` → Concrete. See `resource_management.md` §3 for the full mechanism and behavior hooks.

Unknown prefix falls back to a default material with a warning at load time.

---

## PRL Compilation

### Compiler pipeline

```
parse .map → BSP construction → brush-side projection → portal generation → exterior leaf culling → geometry → BVH → lightmap bake → SH volume bake → pack .prl
```

1. **Parse.** Extracts brush volumes, brush sides, and entities. Applies coordinate transform (Quake Z-up → engine Y-up) and unit scale. Light entities route to FGD translation and validation; they don't participate in BSP construction.
2. **BSP construction.** Partitions world space into solid and empty leaves using brush-derived planes. Leaf solidity is established during construction from the brush half-space intersection — not inferred from face positions afterward.
3. **Brush-side projection.** Derives visible world faces from brush sides. Produces triangulated geometry per empty leaf; faces in solid space are discarded.
4. **Portal generation.** Clips splitting-plane polygons against ancestor planes to produce convex portals connecting adjacent empty leaves. Always runs; portals are stored in every PRL for runtime traversal.
5. **Exterior leaf culling.** Flood-fills through the portal graph from outside the map boundary. Exterior-reachable leaves produce no geometry. A map with a leak has interior leaves incorrectly classified as exterior.
6. **Geometry.** Fan-triangulates faces into a global vertex/index buffer. Associates each face with a material bucket and cell ID.
7. **BVH.** Builds a global SAH BVH over all static geometry organized by `(face, material_bucket)` pair. Flattens to dense arrays; leaves sorted by material bucket for contiguous per-bucket indirect draw slots.
8. **Lightmap bake.** UV-unwraps world geometry into a lightmap atlas. Ray-casts per-texel irradiance and dominant incoming light direction from all static lights against the global BVH. Atlas dimensions are bounded; on overflow the baker retries at a coarser texel density (halving resolution) a bounded number of times before failing the build. Each retry emits a warning — the fallback is visible in logs, not silent. Skipped when the map has no static lights.
9. **Pack.** Writes all sections to the `.prl` binary format.

### PRL section IDs

| Section | ID | When present |
|---------|-----|-------------|
| BspNodes | 12 | Always |
| BspLeaves | 13 | Always |
| Portals | 15 | Always |
| TextureNames | 16 | Always |
| Geometry | 17 | Always |
| AlphaLights | 18 | Always |
| Bvh | 19 | Always |
| ShVolume | 20 | When compiled with lighting |
| LightInfluence | 21 | When compiled with lighting |
| Lightmap | 22 | Always (placeholder atlas when a map has no static lights) |
| ChunkLightList | 23 | Always; per-chunk static-light index lists for specular culling |
| AnimatedLightChunks | 24 | When compiled with animated lights |
| AnimatedLightWeightMaps | 25 | When compiled with animated lights; per-texel weight maps for the compose pass |
| LightTags | 26 | When at least one light carries a tag; one space-delimited tag-list string per AlphaLight record (empty string = untagged) |
| DeltaShVolumes | 27 | When the map has at least one animated light; per-light delta SH probe grids |
| DataScript | 28 | When `data_script` KVP present on `worldspawn`; compiled script bytes + original source path |
| MapEntity | 29 | When the map has at least one non-light, non-worldspawn entity; per-entity classname, origin, angles, tags, and KVP bag for runtime classname dispatch |

### Runtime visibility

Portal traversal is the sole visibility path: per-frame flood-fill from the camera leaf with frustum narrowing at each portal. The runtime falls back to per-leaf AABB frustum culling for solid-leaf, exterior-camera, and no-portals cases. See `rendering_pipeline.md` §2.

---

## Non-Goals

- Runtime level compilation
- WAD file support
- Runtime lightmap baking
