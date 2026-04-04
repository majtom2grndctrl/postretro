# Build Pipeline

> **Read this when:** setting up the map authoring toolchain, modifying the asset pipeline, adding custom entities, or debugging map compilation issues.
> **Key invariant:** engine never modifies ericw-tools output. We consume baked BSP data and supplement with authored metadata through TrenchBroom entities and texture naming conventions.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md)

---

## Pipeline Overview

```
TrenchBroom (.map)
    │
    ▼
ericw-tools (qbsp → vis → light)
    │
    ▼
BSP2 file (.bsp)
    │
    ▼
Engine loads BSP + PNGs at runtime
```

Author maps in TrenchBroom using the custom Postretro game configuration. Compile with ericw-tools. Engine loads the resulting BSP2 file, resolves entities, and loads PNG textures by name at runtime.

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

## Non-Goals

- Extending or forking ericw-tools
- Runtime BSP compilation
- WAD file support
- Runtime lightmap baking
- Custom BSP compiler
