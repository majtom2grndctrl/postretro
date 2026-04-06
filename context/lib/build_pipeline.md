# Build Pipeline

> **Read this when:** setting up the map authoring toolchain, modifying the asset pipeline, adding custom entities, or debugging map compilation issues.
> **Key invariant:** all maps are authored in TrenchBroom. Two compilation paths exist: BSP (ericw-tools) and PRL (prl-build). Engine loads either format.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md)

---

## Pipeline Overview

All maps start as TrenchBroom `.map` files. Two compilation paths:

```
TrenchBroom (.map)
    │
    ├──► ericw-tools (qbsp → vis → light)  ──► BSP2 file (.bsp)
    │
    └──► prl-build (postretro-level-compiler)──► PRL file (.prl)

Engine loads either format + PNGs at runtime
```

**BSP path** (current, stable): ericw-tools compiles geometry, visibility, and lighting into a standard BSP2 file. Engine loads via qbsp crate.

**PRL path** (in development): prl-build bakes geometry, cluster-based visibility, and future sections (lighting, nav mesh, audio) into a custom binary format. Engine loads via postretro-level-format crate. See `plans/prl-spec-draft.md` for the full format spec.

Both paths share the same TrenchBroom authoring workflow, FGD entity definitions, and PNG texture pipeline.

---

## ericw-tools Compilation

Version: **2.0.0-alpha**. Three tools run in sequence:

| Step | Command | Purpose |
|------|---------|---------|
| 1 | `qbsp -bsp2 -notex -wrbrushes map.map` | Compile geometry into BSP2 format. No embedded textures. Emit BRUSHLIST BSPX lump for collision. |
| 2 | `vis map.bsp` | Compute potentially visible set (PVS) data. |
| 3 | `light -bspx -lightgrid map.bsp` | Calculate colored lightmaps, directional lightmaps, and light grid. |

### qbsp

`-bsp2` selects BSP2 format. `-notex` omits texture pixel data from the BSP — engine loads PNGs at runtime instead. `-wrbrushes` writes a `BRUSHLIST` BSPX lump containing convex brush hulls for collision detection — enables arbitrary collision sizes, not limited to Q1's fixed hull system. Do not use `-wrbrushesonly` — that drops clipnodes, removing the fallback collision path.

qbsp typically auto-adds the map file's parent directory as a texture search path — if `textures/` sits alongside the `.map` file, no extra flags are needed. If not, pass `-path <dir>` to point at the textures directory. qbsp reads PNGs for dimensions only (needed for UV mapping).

### vis

No special flags. Computes PVS data used by the engine for visibility culling.

### light

`-bspx` writes extended lighting data as BSPX lumps: colored lightmaps (`RGBLIGHTING`), directional lightmaps (`LIGHTINGDIR`).

`-lightgrid` writes a `LIGHTGRID_OCTREE` BSPX lump — volumetric light probes for lighting sprites and particles.

**Ambient occlusion:** enabled via worldspawn key `_dirt 1` in the map file (or `-dirt 1` CLI override). AO data bakes directly into lightmap samples — no separate lump.

**Caveat:** `-lightgrid` was developed primarily for Quake 2. Its behavior with Q1 BSP2 is experimental. Verify early in development that it produces usable probe data. If output is unusable, fall back to nearest-lightmap sampling or ambient plus nearest-light approximation.

---

## Build Profiles

Different stages of level development need different compilation fidelity. Fast iteration during layout and blockout matters more than final lighting quality. Full compilation matters for testing the final result.

| Profile | qbsp | vis | light | Use case |
|---------|------|-----|-------|----------|
| **Fast** | `qbsp -bsp2 -notex -wrbrushes map.map` | skip | skip | Geometry blockout, collision testing. No vis, no lighting. |
| **Draft** | `qbsp -bsp2 -notex -wrbrushes map.map` | `vis map.bsp` | `light -bspx map.bsp` | Layout iteration with basic lighting. No light grid. |
| **Full** | `qbsp -bsp2 -notex -wrbrushes map.map` | `vis map.bsp` | `light -bspx -lightgrid map.bsp` | Final quality. All BSPX lumps, full vis, light grid. |

The engine handles missing data gracefully at each level: no PVS → draw everything (slower but correct), no lightmaps → flat white lighting, no light grid → fallback sprite lighting. Build profiles are a content workflow concern — the engine makes no assumptions about which profile produced the BSP.

---

## BSP2 Format

BSP2 removes the geometry limits of BSP29: 65K face cap, 32K clipnode cap, and +/-32K coordinate range. No downside for a custom engine. The qbsp Rust crate auto-detects BSP29 vs. BSP2 at load time.

---

## PNG Texture Pipeline

No WAD files. Textures are authored as PNGs.

| Stage | What happens |
|-------|-------------|
| Author | Create PNGs in `textures/<collection>/<name>.png`. TrenchBroom requires one subdirectory level. |
| TrenchBroom | Displays textures via the Postretro game configuration, which points at the textures directory. |
| qbsp | Reads PNGs for dimensions only (`-notex` omits pixel data). |
| BSP output | Stores texture headers: name and dimensions. No pixel data. |
| Engine | Loads PNGs at runtime, matched to BSP texture entries by name string. |

---

## TrenchBroom Game Configuration

Custom `Postretro` game config in standard TrenchBroom format. Two responsibilities:

- Points at the textures directory so TrenchBroom displays PNGs in the texture browser.
- References the custom FGD file for entity definitions.

---

## Custom FGD

Project deliverable alongside the engine. Defines Postretro-specific entities for TrenchBroom.

| Entity | Type | Purpose | Properties |
|--------|------|---------|------------|
| `env_fog_volume` | brush | Per-region fog | `color`, `density`, `falloff` |
| `env_cubemap` | point | Reflection probe position | `size` (resolution per face; default 256) |
| `env_reverb_zone` | brush | Acoustic zone | `reverb_type`, `decay_time`, `occlusion_factor` |

### Entity resolution

- **`env_fog_volume`** — resolved to BSP leaves at load time. Each leaf in the volume gets per-leaf atmospheric haze parameters.
- **`env_cubemap`** — marks a position for offline cubemap baking. Bake tool is out of initial scope.
- **`env_reverb_zone`** — resolved to BSP leaves at load time. Each leaf in the volume gets spatial reverb parameters for the audio subsystem.

---

## Surface Material Derivation

Texture name prefix maps to a material enum. Drives footstep sounds, bullet impacts, and decals. The engine provides the prefix-to-material lookup mechanism; which prefixes exist is a game content concern. The table grows as content requires it.

Example: `metal_floor_01` → Metal, `concrete_wall_03` → Concrete. See `resource_management.md` §3 for the full mechanism and behavior hooks.

Unknown prefix falls back to a default material with a warning at load time.

---

## Baked Data Summary

| Data | Source | How |
|------|--------|-----|
| Colored lightmaps | ericw-tools (`light -bspx`) | RGBLIGHTING BSPX lump |
| Directional lightmaps | ericw-tools (`light -bspx`) | LIGHTINGDIR BSPX lump — per-pixel specular |
| Ambient occlusion | ericw-tools (worldspawn `_dirt 1`) | Baked into lightmap data |
| Volumetric light probes | ericw-tools (`light -lightgrid`) | LIGHTGRID_OCTREE BSPX lump (experimental for Q1) |
| Brush collision hulls | ericw-tools (`qbsp -wrbrushes`) | BRUSHLIST BSPX lump — convex hulls for collision |
| Surface material types | Texture naming convention | Prefix lookup table |
| Fog volumes | FGD entity (`env_fog_volume`) | Brush entity resolved to BSP leaves at load time |
| Reflection probes | FGD entity (`env_cubemap`) | Point entity — offline cubemap bake |
| Acoustic zones | FGD entity (`env_reverb_zone`) | Brush entity resolved to BSP leaves at load time |

---

## PRL Compilation

The PRL compiler (`prl-build`) reads `.map` files directly via shambler and produces `.prl` binary level files. It replaces ericw-tools' three-step pipeline with a single tool.

### Compiler pipeline

```
parse .map → voxelize brushes → spatial grid (cell classification) → PVS (ray-cast) → geometry → pack .prl
```

1. **Parse.** Shambler extracts brush volumes, faces, and entities from the `.map` file.
2. **Voxelize.** Brush volumes are rasterized into a 3D solid/empty bitmap (compile-time only, not stored in output). This enables point-in-solid classification and efficient ray occlusion testing.
3. **Spatial grid.** Faces are assigned to uniform grid cells by centroid. Cells are classified as solid, air, or boundary using the voxel bitmap. Solid cells are discarded. Boundary cells (straddling walls) are subdivided. Air cells are merged into their nearest face-containing cell (expanding its bounds for camera containment).
4. **PVS.** Ray-cast visibility between cluster pairs using 3D-DDA ray marching through the voxel grid. Sample points in solid space are rejected. Adjacent clusters are always mutually visible.
5. **Geometry.** Faces are fan-triangulated into vertex/index buffers in engine-native Y-up coordinates.
6. **Pack.** Geometry and visibility sections are written to the `.prl` binary format.

### Key differences from the BSP path

- **Cluster-based visibility** instead of per-leaf BSP PVS. No BSP tree — the compiler uses a voxel grid for spatial queries.
- **Engine-native coordinates** (Y-up). No runtime coordinate transform.
- **Section-based binary format** with independent versioning. New data types (lighting, nav mesh, audio) are added as sections without breaking existing levels.
- **Self-describing levels.** Everything the engine needs is in one `.prl` file — no secondary data files or string parsing at load time.

Full spec: `plans/prl-spec-draft.md`.

---

## Non-Goals

- Extending or forking ericw-tools
- Runtime level compilation
- WAD file support
- Runtime lightmap baking
