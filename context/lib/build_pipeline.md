# Build Pipeline

> **Read this when:** setting up the map authoring toolchain, modifying the asset pipeline, adding custom entities, or debugging map compilation issues.
> **Key invariant:** maps are authored in TrenchBroom. Engine canonical unit: 1 unit = 1 meter. PRL is the primary compilation target; BSP loading remains for legacy asset support.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md)

---

## Pipeline Overview

Maps are authored in TrenchBroom, compiled to PRL with prl-build:

```
TrenchBroom (.map) ──► prl-build (postretro-level-compiler) ──► PRL file (.prl)

Engine loads PRL + PNGs at runtime
```

**PRL path (primary):** prl-build builds a BSP tree, generates portal geometry, and packs geometry into a custom binary format. Default mode stores portal geometry for runtime traversal; `--pvs` mode computes a precomputed PVS instead. Engine loads via the postretro-level-format crate.

**BSP path (legacy support):** Engine can load `.bsp` files compiled by ericw-tools. No active development on this path. See §BSP below.

Both paths share the TrenchBroom authoring workflow, FGD entity definitions, and PNG texture pipeline.

---

## BSP (Legacy Support)

Engine loads `.bsp` files via the qbsp crate. BSP2 format (removes BSP29 geometry limits). No active development on this path — it exists to load existing assets while content migrates to PRL.

Existing BSP files compiled with ericw-tools continue to render via the BSP loader. New levels should target PRL.

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
| Geometry | prl-build (CSG clip → BSP → pack) | Geometry section |
| BSP tree | prl-build | BspNodes + BspLeaves sections |
| Visibility | prl-build | Portals section (default) or LeafPvs section (`--pvs`) |
| Surface material types | Texture naming convention | Prefix lookup table |
| Lighting | prl-build (Phase 4 — see `plans/roadmap.md`) | PRL-native sections, designed in Phase 4 |
| Fog volumes | FGD entity (`env_fog_volume`) | Brush entity resolved to BSP leaves at load time |
| Reflection probes | FGD entity (`env_cubemap`) | Point entity — offline cubemap bake |
| Acoustic zones | FGD entity (`env_reverb_zone`) | Brush entity resolved to BSP leaves at load time |

---

## PRL Compilation

The PRL compiler (`prl-build`) reads `.map` files directly via shambler and produces `.prl` binary level files. It replaces ericw-tools' three-step pipeline with a single tool.

> **Pipeline restructure planned.** The compile pipeline below is being restructured to a brush-volume-first BSP construction with face extraction at the tail (`parse → brush-volume BSP → face extraction → portal generation → portal vis → geometry → pack`). CSG face clipping disappears as a discrete stage and leaf solidity becomes structural, established during construction rather than post-hoc. See `plans/drafts/brush-volume-bsp/`. The text below describes the current implementation; this section will be rewritten when the refactor lands.

### Compiler pipeline

```
parse .map → CSG face clipping → BSP compilation → portal generation → portal vis → geometry → pack .prl
```

1. **Parse.** Shambler extracts brush volumes, faces, and entities. Two transforms are applied at the parse boundary: (a) axis swizzle (Quake Z-up → engine Y-up) and (b) unit scale (idTech2: 0.0254 m/unit, exact). Vertex positions, entity origins, and plane distances are converted to engine meters; plane normals receive the swizzle only (direction vectors — scale must not be applied). The scale comes from a single map-format source, never duplicated at call sites. All downstream stages receive engine-native coordinates in meters.
2. **CSG face clipping.** Each face is clipped against all brush volumes using Sutherland-Hodgman polygon clipping. Faces that lie entirely inside a solid brush are discarded; faces that partially overlap are trimmed to the exterior portion. An AABB pre-filter skips brush pairs with non-overlapping bounds. This eliminates z-fighting at shared surfaces between adjacent brushes — the same problem BSP solves structurally via splitting, done here as an explicit compile-time step. A face on its own brush's boundary plane is not clipped (it sits on the plane, not behind all half-planes).
3. **BSP compilation.** Builds a BSP tree from world faces. Produces interior nodes (splitting planes) and leaves (convex regions). Leaf solidity is derived from brush ownership: face normals point outward from their source brush, so any leaf containing a face lies on that brush's air side and is empty; faceless leaves are solid. Solid leaves represent brush interiors. Empty leaves represent navigable space. (A brush-volume-first BSP construction that establishes solidity structurally during construction is planned — see `context/plans/drafts/brush-volume-bsp/`.)
4. **Portal generation.** For each BSP internal node, clips the splitting-plane polygon against ancestor splitting planes to produce the portal polygon bounding that node's partition. Each portal is a convex polygon connecting two adjacent empty leaves. In default mode, portals are stored in the `.prl` file (section 15) for runtime traversal. In `--pvs` mode, portals are used as intermediate data and discarded.
5. **Portal vis** (`--pvs` mode only). Per empty leaf, floods through the portal graph. A leaf L' is potentially visible from L if any sequence of portals connects them. Output: per-leaf PVS bitsets, RLE-compressed. Computed in parallel (one task per leaf).
6. **Geometry.** Fan-triangulates faces into vertex/index buffers. Faces grouped by leaf index for efficient per-leaf draw calls.
7. **Pack.** Writes BSP tree nodes, BSP leaves (face ranges, bounds), and geometry to the `.prl` binary format. Default mode also writes the Portals section (15). `--pvs` mode writes the LeafPvs section (14) instead.

### PRL section IDs

| Section | ID | When present |
|---------|-----|-------------|
| Geometry | 1 | Legacy (pre-texture support) |
| GeometryV2 | 3 | Always (position + UV vertices, texture index per face) |
| BspNodes | 12 | Always |
| BspLeaves | 13 | Always |
| LeafPvs | 14 | `--pvs` mode only |
| Portals | 15 | Default mode |
| TextureNames | 16 | Always (deduplicated texture name list) |

### Runtime visibility

Two paths, selected by which PRL section is present.

| PRL section present | Runtime path | Notes |
|---------------------|--------------|-------|
| Portals (15) | Per-frame portal flood-fill with frustum narrowing | Default. Handles corners and narrow apertures without precomputation. |
| LeafPvs (14) | Precomputed PVS bitset lookup | Fallback for `--pvs` builds. |

**Architectural stance: id Tech 4 (Doom 3, 2004), not Quake 1.** Visibility is computed per frame from portal geometry, not baked into a precomputed bitset. The reasoning matches Carmack's break from Quake's vis pipeline: precomputed PVS lengthens compile cycles, fights with dynamic geometry, and the per-frame cost is trivial at modern leaf counts. The compiler still supports `--pvs` so a precomputed fallback exists, but the default path is runtime traversal.

#### Portal traversal (default path)

Single-pass portal flood-fill with polygon-vs-frustum clipping at each hop. This is the id Tech 4 (Doom 3, Quake 4, Prey) form of runtime portal vis.

For each portal visited by the BFS:

1. **Clip the portal polygon against the current frustum** using Sutherland-Hodgman. An empty clip output (fewer than 3 vertices after clipping) is the unified rejection signal — the portal is entirely outside the current sight cone.
2. **Narrow the frustum through the clipped polygon.** The new frustum is built from the portal plane (near), one edge plane per clipped edge through the camera position, and the far plane carried from the current frustum.
3. **Enqueue the neighbor leaf** with the narrowed frustum. Solid leaves block traversal.

**Strict-subset invariant.** Because the clipped polygon lies entirely inside the current frustum by construction, the edge planes derived from it form a cone strictly inside the current cone. By induction from the camera's initial frustum, every narrowed frustum reachable through any portal chain is a strict subset of the camera frustum, and every leaf marked visible by the flood-fill lies inside the camera's view cone.

There is no separate per-leaf AABB frustum cull on this path. The clip-and-narrow step both tests visibility and builds the next frustum in one operation, and the strict-subset invariant makes a second enforcement pass redundant.

Floating-point clipping uses a small inclusive epsilon at half-space boundaries (over-inclusion at the boundary cannot violate the invariant — any genuinely-outside slop is discarded by the next hop's edge planes). Degenerate clipped polygons — those that touch the frustum only at a single point or edge — take the same "not visible" rejection path as the empty case.

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
