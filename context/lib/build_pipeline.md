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

**PRL path** (in development): prl-build builds a BSP tree, generates portal geometry, and packs geometry into a custom binary format. Default mode stores portal geometry for runtime traversal; `--pvs` mode computes a precomputed PVS instead. Future sections: lighting, nav mesh, audio. Engine loads via postretro-level-format crate.

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
parse .map → BSP compilation → portal generation → portal vis → geometry → pack .prl
```

1. **Parse.** Shambler extracts brush volumes, faces, and entities. Coordinate transform (Quake Z-up → engine Y-up) applied at the parse boundary. All downstream stages receive engine-native coordinates.
2. **BSP compilation.** Builds a BSP tree from world faces. Produces interior nodes (splitting planes) and leaves (convex regions). Leaves classified solid or empty via brush half-plane test. Solid leaves represent brush interiors. Empty leaves represent navigable space.
3. **Portal generation.** For each BSP internal node, clips the splitting-plane polygon against ancestor splitting planes to produce the portal polygon bounding that node's partition. Each portal is a convex polygon connecting two adjacent empty leaves. In default mode, portals are stored in the `.prl` file (section 15) for runtime traversal. In `--pvs` mode, portals are used as intermediate data and discarded.
4. **Portal vis** (`--pvs` mode only). Per empty leaf, floods through the portal graph. A leaf L' is potentially visible from L if any sequence of portals connects them. Output: per-leaf PVS bitsets, RLE-compressed. Computed in parallel (one task per leaf).
5. **Geometry.** Fan-triangulates faces into vertex/index buffers. Faces grouped by leaf index for efficient per-leaf draw calls.
6. **Pack.** Writes BSP tree nodes, BSP leaves (face ranges, bounds), and geometry to the `.prl` binary format. Default mode also writes the Portals section (15). `--pvs` mode writes the LeafPvs section (14) instead.

### PRL section IDs

| Section | ID | When present |
|---------|-----|-------------|
| Geometry | 1 | Always |
| BspNodes | 12 | Always |
| BspLeaves | 13 | Always |
| LeafPvs | 14 | `--pvs` mode only |
| Portals | 15 | Default mode |

### Runtime visibility

When a `.prl` file contains a Portals section (15), the engine walks the portal graph per frame with frustum clipping. This is the preferred path: visibility naturally handles corners and narrow apertures without precomputation. When only a LeafPvs section (14) is present, the engine falls back to precomputed PVS culling.

### Key differences from the former voxel approach

- No voxel grid. Solid/empty classification uses brush half-plane geometry directly.
- Leaf-based visibility replaces cluster-based PVS. BSP leaves are the visibility units.
- BSP tree stored in `.prl` — enables O(log n) point-in-leaf at runtime.
- Portal geometry stored in `.prl` by default — enables per-frame frustum-clipped portal traversal.

---

## Non-Goals

- Extending or forking ericw-tools
- Runtime level compilation
- WAD file support
- Runtime lightmap baking
