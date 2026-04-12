# Compile Target Ideas: Black Ops 3 and rbdoom-3-bfg

> **Read this when:** designing PRL compile targets, planning Phase 4.5 baker, or evaluating what baked data the engine should produce and consume.
> **Key question:** what can Postretro bake at compile time to deliver modern indirect lighting without runtime raytracing, excessive load times, or per-frame GPU cost?

---

## Sources

Two engines with relevant compile pipelines:

- **Call of Duty: Black Ops 3** (Treyarch, 2015). IW engine descendant. Switched to deferred PBR. Shipped mod tools (Radiant Black). Extensive precomputation: lightmaps, irradiance volumes, reflection probes, sparse shadow trees, Umbra occlusion. Published at SIGGRAPH 2016 by Activision Research.
- **rbdoom-3-bfg** (Robert Beckebans). Modernized id Tech 4 fork. Added PBR, baked GI via irradiance volumes and environment probes, shadow atlas, SSAO, Vulkan/DX12 via NVRHI. Open-source. BSP + portal architecture closest to Postretro's.

---

## Compile Pipelines Compared

Both engines separate compilation into independent passes. Each pass can run alone for fast iteration.

| Pass | Black Ops 3 | rbdoom-3-bfg | Postretro (current) |
|------|-------------|--------------|---------------------|
| Geometry / BSP | `cod2map64` → `.d3dbsp` | `dmap` → `.proc` + `.cm` + `.aas` | `prl-build` → `.prl` |
| Lighting | Offline GI raytracer → lightmaps + light grid | `bakeLightGrids` → `.lightgrid` + atlas EXRs | Phase 4.5 (planned) |
| Reflection probes | Offline cubemap render → probe cubemaps in `.d3dbsp` | `bakeEnvironmentProbes` → octahedron EXRs | `env_cubemap` entity defined; bake not implemented |
| Visibility | Umbra voxelization → occlusion tome | BSP portals + Intel MOC (runtime) | BSP portals (runtime) |
| Packaging | Linker → `.ff` FastFile | Standard id Tech 4 asset loading | Single `.prl` binary |

**Key insight: separate passes.** BO3's "onlyents" mode recompiles entity data without touching geometry or lighting. rbdoom-3-bfg bakes light grids and probes as standalone commands. Both allow lighting iteration without full recompile. Postretro's `prl-build` currently runs geometry and BSP in one pass. Phase 4.5 should be a separate pass or a late stage that can run independently.

---

## Irradiance Volumes (Light Grids)

Both engines converged on the same solution for indirect diffuse lighting: sparse 3D grids of spherical harmonic probes, one grid per spatial region.

### Black Ops 3

Replaced traditional lightmaps with irradiance volumes stored as hardware-filtered 3D textures (SIGGRAPH 2016, JT Hooker). Artists place irradiance volumes and author convex clipping volumes to prevent light leaks. Benefits:

- Unified lighting — same data lights BSP, models, particles, characters.
- Eliminates per-face lightmap UVs. No lightmap atlas packing.
- Cheap runtime sample: single 3D texture lookup with trilinear filtering.
- Faster bake than per-texel lightmaps.

### rbdoom-3-bfg

Per-BSP-area 3D grids. Default spacing: 64×64×128 units. Max 16384 probes per area. L4 spherical harmonics encoded as octahedrons. Trilinear interpolation of nearest 8 grid points. Supports bounce lighting (default 1 bounce). Produces `.lightgrid` file and atlas textures.

### Relevance to Postretro

Irradiance volumes are the natural form for Phase 4.5 baked lighting. Advantages over per-face lightmaps:

- **No lightmap UV generation.** Lightmap atlasing is a hard problem — packing, padding, chart splitting, per-face parameterization. Irradiance volumes skip all of it.
- **No lightmap texture memory.** A 3D grid with moderate spacing (e.g., 64×64×128 units) stores thousands of probes in kilobytes of SH data. A lightmap atlas for the same level can be megabytes.
- **Unified lighting for all object types.** Static geometry, sprites, and future dynamic objects sample the same grid. No separate light-grid-for-models system.
- **Simpler runtime.** Sample = find enclosing cell, trilinear-interpolate 8 SH probe values, evaluate SH at surface normal. One texture lookup or buffer read per fragment.
- **Fits the aesthetic.** Retro shooters don't need per-texel shadow detail. Smooth, low-frequency indirect lighting from irradiance volumes matches the visual target. Direct lights (Phase 5 dynamic lights) add local detail where needed.

Tradeoff: irradiance volumes cannot represent sharp indirect shadows (light around a doorframe, for example). Both BO3 and rbdoom-3-bfg solve this with clipping volumes (BO3) or per-area grids (rbdoom-3-bfg) to prevent light leaking through walls. Postretro's BSP areas (portal-connected groups of leaves) are a natural boundary for per-area grids.

**Possible PRL section:** `IrradianceGrid` — per-BSP-area 3D grids. Each grid cell stores SH coefficients (L2 is 9 coefficients × 3 color channels = 27 floats; L1 is 4×3 = 12 floats). Grid metadata: origin, cell size, grid dimensions per area.

---

## Environment Probes (Specular IBL)

Both engines bake cubemaps at marked positions for specular reflections.

### Black Ops 3

Probes rendered at compile time as 6-face cubemaps. **Normalized**: divided by average diffuse lighting at capture point. At runtime, multiplied by per-pixel reconstructed diffuse from lightmap/light grid. This lets one probe serve a large area with varying lighting intensity — only the reflection *shape* varies, not brightness.

### rbdoom-3-bfg

One probe per BSP area (auto-placed at area center), or manually via `env_probe` entities. L4 SH for diffuse. GGX-convolved mip chain for specular (Split Sum Approximation, Karis 2013). Octahedron encoding. Stored as EXR, cached to BC6H via ISPC Texture Compressor.

### Relevance to Postretro

Postretro already defines `env_cubemap` entities. Two levels of implementation:

1. **Minimal (fits retro aesthetic):** Bake low-resolution cubemaps (64–128 per face). Store unfiltered. Sample at a single mip for rough reflections on metal, water, glass. No convolution pipeline needed. Cheap to bake, cheap to store, adds environmental presence.

2. **Enhanced (if specular quality matters):** Bake at higher resolution (256 per face). Convolve mip chain for roughness-dependent reflections. Store as BC6H-compressed octahedrons. More bake time, more storage, noticeably better metal and water surfaces.

BO3's normalization trick is worth adopting regardless of quality tier. It decouples probe brightness from bake position, so fewer probes cover more area.

**Possible PRL section:** `ReflectionProbes` — per-probe cubemap data (or octahedron atlas), probe world position, influence radius. Separate from irradiance grid.

---

## Baked Shadow Data

### Black Ops 3: Sparse Shadow Trees

Kevin Myers, SIGGRAPH 2016. Baked sun shadowmaps for entire levels, compressed into sparse voxel octrees. Each node encodes light visibility. Common subtrees merged. 1000× compression ratios. Runtime: SST shadows far objects; dynamic shadow maps override near the camera.

### rbdoom-3-bfg: Precomputed Shadow Volumes

For each static light × static geometry pair, dmap precomputes stencil shadow volumes. Stored in `.proc` as shadow models. At runtime, only dynamic lights need shadow computation.

### Relevance to Postretro

Baked shadows add depth without runtime shadow maps. Two approaches that fit the retro aesthetic:

1. **Per-face sun occlusion factor.** At bake time, cast rays from each face centroid toward the sun direction. Store a 0–1 occlusion scalar per face (or per vertex). Simplest form: binary lit/unlit. Runtime cost: multiply base lighting by the occlusion factor. Storage: one byte per face or per vertex. No shadow maps, no stencil volumes.

2. **Per-probe directional occlusion.** Extend the irradiance grid probes to store visibility in the dominant light direction. At bake time, when computing SH for each probe, also record how much of the sun's solid angle is visible. Runtime: modulate the sun contribution per probe. Slightly more sophisticated than per-face but uses the same grid.

Full sparse shadow trees or precomputed stencil volumes add complexity Postretro likely doesn't need. The per-face or per-probe approach achieves 80% of the visual effect at 5% of the implementation cost.

---

## Light Grid: Primary Light Assignment

### Black Ops 3

The light grid stores a **primary light choice per cell** — which shadow-casting light most affects each grid point. Runtime shadow maps render only for the primary light at each surface's grid cell. Other lights contribute unshadowed.

### Relevance to Postretro

When Phase 5 adds dynamic lights, the irradiance grid can store a "dominant light" index per cell. Useful for:

- Choosing which light casts dynamic shadows (if ever implemented).
- Driving sprite lighting direction (billboard sprites lit from the dominant light direction).
- Selecting which light produces specular highlights on nearby surfaces.

Low-cost addition to the grid data: one u16 index per cell.

---

## Practical Compile Target Summary

Ordered by implementation priority and complexity.

| Compile target | PRL section | Bake cost | Runtime cost | Visual payoff |
|----------------|-------------|-----------|--------------|---------------|
| Irradiance volumes | `IrradianceGrid` | Moderate (raytrace SH per probe) | Low (trilinear SH lookup per fragment) | High — indirect lighting on all surfaces |
| Per-face sun occlusion | Extend `GeometryV2` or new section | Low (raycast per face) | Trivial (multiply per face) | Medium — baked sun shadows |
| Reflection probes (minimal) | `ReflectionProbes` | Low (render 6 faces per probe) | Low (single cubemap sample) | Medium — environmental reflections on metal/water |
| Dominant light index | Embed in `IrradianceGrid` | Trivial (select brightest during bake) | Trivial (one index read) | Low but enables future features |
| Reflection probes (convolved) | `ReflectionProbes` | Moderate (convolve mip chain) | Low (mip-selected sample) | High — roughness-dependent reflections |
| Baked shadow trees | New section | High (voxelize + compress) | Low (tree traversal) | High but complex to implement |

---

## What Doesn't Fit

- **Deferred rendering.** Both BO3 and rbdoom-3-bfg use or support deferred. Postretro's forward pipeline is correct for the target light count and retro aesthetic. Already a stated non-goal.
- **Full PBR material pipeline.** rbdoom-3-bfg's RMAO maps and GGX shading are overkill. Baked indirect + forward dynamic lights + simple specular achieves the target look.
- **Runtime shadow atlas.** rbdoom-3-bfg renders all visible lights into a 16K shadow atlas per frame. Too expensive for retro targets. Baked shadow data plus a small dynamic shadow budget (one or two shadow maps for gameplay lights) is sufficient.
- **Umbra-style precomputed occlusion.** Portal traversal already solves visibility. Adding a separate occlusion system duplicates work without benefit at retro-scale geometry counts.
- **Software occlusion culling (Intel MOC).** Same reasoning. Portal traversal is the primary visibility path and handles the workload.

---

## Editor and Format Considerations

### TrenchBroom and Ultimate Doom Builder

Both editors support entity definitions that map to compile targets:

- **Light entities** already defined in the FGD (`light`, `light_spot`, `light_sun`). These drive irradiance volume baking.
- **`env_cubemap`** already defined. Drives reflection probe baking.
- **Grid density control:** a worldspawn key (e.g., `_lightgrid_size "64 64 128"`) lets mappers override default irradiance grid spacing per map. Both BO3 and rbdoom-3-bfg support this. TrenchBroom and UDB both handle worldspawn keys natively.
- **Light leak prevention:** brush entities or texture-based clip volumes that block irradiance propagation. BO3 uses artist-authored convex volumes. rbdoom-3-bfg uses BSP area boundaries. Postretro's portal-connected areas provide natural boundaries without extra authoring.

### Valve 220 and UDMF

Both input formats carry the entity data needed for baking:

- **Valve 220** — entity key-value pairs for lights, probe positions, grid overrides. UV data for texture projection. No format-level obstacles.
- **UDMF** — richer key-value system. Arbitrary per-entity and per-linedef properties. Can encode everything Valve 220 can plus additional metadata (e.g., per-face lightmap scale, per-sector light level as a hint to the baker).

Neither format constrains the compile targets. The map format carries authored intent (light positions, colors, intensities, probe positions). The compiler translates that intent into baked data. Format differences are absorbed at parse time.

---

## Sources

- Activision Research, SIGGRAPH 2016: Volumetric GI (JT Hooker), Sparse Shadow Trees (Kevin Myers), GTAO (Jorge Jimenez)
- Activision Research: Precomputed Lighting in CoD: Infinite Warfare, Modern Warfare, Vanguard
- Lazarov, SIGGRAPH 2011/2013: PBS in Black Ops, Black Ops II
- rbdoom-3-bfg GitHub repository (Robert Beckebans): README, RELEASE-NOTES, source code
- Fabien Sanglard: Doom 3 dmap preprocessing
- The Dark Mod Wiki: PROC file format
- Zeroy Wiki: d3dbsp format, FastFile format, compile tools documentation
- c0de517e: Retrospective on Call of Duty Rendering
- Karis 2013 (Epic): Split Sum Approximation for specular IBL
- McGuire et al., HPG 2012: Scalable Ambient Obscurance
