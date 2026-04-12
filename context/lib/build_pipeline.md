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

**PRL path:** prl-build builds a BSP tree, generates portal geometry, and packs geometry into a custom binary format. Default mode stores portal geometry for runtime traversal; `--pvs` mode computes a precomputed PVS instead. Engine loads via the postretro-level-format crate.

The PRL path uses the TrenchBroom authoring workflow, FGD entity definitions, and PNG texture pipeline.

---

## Supported Map Formats

prl-build accepts `.map` files. The `--format` flag selects the dialect; default is `idtech2`.

| Dialect | Status | Unit scale | Notes |
|---------|--------|-----------|-------|
| idTech2 (Quake 1/2) | Supported | 1 unit = 0.0254 m (inch) | Default. Parsed via shambler/shalrath. |
| idTech3 (Quake 3) | Not yet supported | — | Bezier patches. |
| idTech4 (Doom 3) | Not yet supported | — | meshDef / brushDef3. |

**Texture projection:** both Standard (axis-aligned) and Valve 220 (explicit UV axes) projection formats are supported end-to-end. Shalrath auto-detects the format per face — they can coexist in one `.map` file, which is what TrenchBroom produces. UV computation handles both variants during geometry extraction.

---

## PNG Texture Pipeline

No WAD files. Textures are authored as PNGs.

| Stage | What happens |
|-------|-------------|
| Author | Create PNGs in `textures/<collection>/<name>.png`. TrenchBroom requires one subdirectory level. |
| TrenchBroom | Displays textures via the Postretro game configuration, which points at the textures directory. |
| prl-build | Reads PNGs for dimensions during compilation. |
| PRL output | TextureNames section stores a deduplicated texture name list. No pixel data. |
| Engine | Loads PNGs at runtime, matched to PRL texture entries by name string. |

---

## TrenchBroom Game Configuration

Custom `Postretro` game config in standard TrenchBroom format. Two responsibilities:

- Points at the textures directory so TrenchBroom displays PNGs in the texture browser.
- References the custom FGD file for entity definitions.

---

## Custom FGD

Project deliverable alongside the engine. Defines Postretro-specific entities for TrenchBroom.

| Entity | Type | Purpose | Key Properties |
|--------|------|---------|------------|
| `light` | point | Omnidirectional light | `light` (intensity), `_color` (RGB), `_fade` (falloff distance), `delay` (falloff model), `style` (animation) |
| `light_spot` | point | Spotlight with cone | + `_cone`, `_cone2` (inner/outer angles), `mangle`/`target` (direction) |
| `light_sun` | point | Directional sun light | + `mangle` (direction vector) |
| `env_fog_volume` | brush | Per-region fog | `color`, `density`, `falloff` |
| `env_cubemap` | point | Reflection probe position | `size` (resolution per face; default 256) |
| `env_reverb_zone` | brush | Acoustic zone | `reverb_type`, `decay_time`, `occlusion_factor` |

### Entity resolution

- **`light`, `light_spot`, `light_sun`** — parsed, translated to canonical format, and validated at compile time. Validation rules: falloff distance required, spotlight direction verified, intensity bounds checked. Canonical lights feed the SH irradiance volume baker and the runtime direct lighting path. Compilation fails on validation errors.
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
| Geometry | prl-build (brush-volume BSP → brush-side projection → pack) | Geometry section — positions, UVs, packed normals, packed tangents |
| BSP tree | prl-build | BspNodes + BspLeaves sections |
| Visibility | prl-build | Portals section (default) or LeafPvs section (`--pvs`) |
| Per-cell draw chunks | prl-build | Face groups keyed by portal cell for runtime indirect draws |
| Surface material types | Texture naming convention | Prefix lookup table |
| Light entities | FGD entities (`light`, `light_spot`, `light_sun`) | Parsed, translated to canonical format. Feeds both the SH baker and the runtime direct-lighting path. |
| Indirect lighting | prl-build (Phase 4) | SH L2 irradiance volume (regular 3D grid) baked from canonical lights; stored in PRL section |
| Fog volumes | FGD entity (`env_fog_volume`) | Brush entity resolved to BSP leaves at load time |
| Reflection probes | FGD entity (`env_cubemap`) | Point entity — offline cubemap bake |
| Acoustic zones | FGD entity (`env_reverb_zone`) | Brush entity resolved to BSP leaves at load time |

---

## PRL Compilation

The PRL compiler (`prl-build`) reads `.map` files directly via shambler and produces `.prl` binary level files.

### Compiler pipeline

```
parse .map → brush-volume BSP construction → brush-side projection → portal generation → exterior leaf culling → portal vis → geometry → pack .prl
```

Light entity parsing and translation happen during the parse stage. Shambler extracts light entities from the `.map` file. The translation layer converts mapper-facing FGD properties to canonical format and validates them (falloff distance required, spotlight direction verified, etc.). Invalid lights fail compilation with a clear error message. Canonical lights are collected and consumed by Phase 4.5 baker; they do not participate in BSP construction.

1. **Parse.** Shambler extracts brush volumes, brush sides, and entities. Two transforms are applied at the parse boundary: (a) axis swizzle (Quake Z-up → engine Y-up) and (b) unit scale (idTech2: 0.0254 m/unit, exact). Vertex positions, entity origins, and plane distances are converted to engine meters; plane normals receive the swizzle only (direction vectors — scale must not be applied). The scale comes from a single map-format source, never duplicated at call sites. All downstream stages receive engine-native coordinates in meters. Brush sides — the textured half-plane polygons bounding each brush — are grouped per brush at parse time; they are the input to BSP construction, not world faces. Light entities are extracted alongside other point entities and passed to the translation layer for validation and canonical format conversion.
2. **Brush-volume BSP construction.** Partitions space by recursively splitting the world AABB with brush-derived planes. Recursion tracks the inside set — the brush indices whose half-spaces fully contain the current region — and terminates at a leaf when the region is uniformly inside one brush set (solid) or uniformly outside every brush (empty). Leaf solidity is structural: it is established during construction, not inferred from face positions afterward. Splitter candidates are drawn from the full set of bounding planes of candidate brushes, including planes no face sits on, so narrow air gaps and adjacent brush boundaries are always detected. The world AABB is the union of brush AABBs with a one-meter slack margin on each axis. Recursion depth is hard-capped; pathological input yields a compiler error rather than a stack overflow.
3. **Brush-side projection.** Derives world faces from brush sides in two passes. Pass 1 walks each brush side down the tree using plane-index equality as the routing primitive (a polygon on a splitting plane goes to one side only, never both), splits on all other planes, and accumulates the fragments that survive into empty leaves as a per-side visible hull. Pass 2 distributes each visible hull back through the tree, emitting a triangulated face in every empty leaf it reaches. Fragments that land in solid leaves are dropped — face-in-solid culling falls out of the walk for free, without a separate clipping stage. When two brush sides on the same oriented plane reach the same leaf, the resolution is containment-aware: a polygon fully contained in another is dropped as redundant, but partially-overlapping coplanar polygons are emitted both and any visible z-fighting is left as an authoring diagnostic. Mismatched textures across a containment-resolved drop are surfaced as a warning. The compiler does not attempt 2D polygon union on coplanar overlaps — by design, partially-overlapping coplanar brushes are an authoring error this stage will not paper over.
4. **Portal generation.** For each BSP internal node, clips the splitting-plane polygon against ancestor splitting planes to produce the portal polygon bounding that node's partition. Each portal is a convex polygon connecting two adjacent empty leaves. In default mode, portals are stored in the `.prl` file (section 15) for runtime traversal. In `--pvs` mode, portals are used as intermediate data and discarded.
5. **Exterior leaf culling.** Flood-fills through the portal graph from a point outside the map's bounding volume. Every empty leaf reachable from outside is an exterior leaf. Exterior leaves produce no packed geometry — void-facing surfaces of the sealing brushes are absent from the output. A map with a leak has interior leaves incorrectly classified as exterior.
6. **Portal vis** (`--pvs` mode only). Per empty leaf, floods through the portal graph. A leaf L' is potentially visible from L if any sequence of portals connects them. Output: per-leaf PVS bitsets, RLE-compressed. Computed in parallel (one task per leaf).
7. **Geometry.** Fan-triangulates faces into vertex/index buffers. Faces grouped by leaf index for efficient per-leaf draw calls.
8. **Pack.** Writes BSP tree nodes, BSP leaves (face ranges, bounds), and geometry to the `.prl` binary format. Default mode also writes the Portals section (15). `--pvs` mode writes the LeafPvs section (14) instead.

### Leaf solidity

Every leaf's solid/empty state is assigned by BSP construction, not by a post-pass over emitted faces. The inside-set invariant means a leaf is known to be inside the intersection of its bounding brushes' half-spaces (solid) or outside all of them (empty) at the moment it is produced. This removes the class of bugs where centroid-based classification on a post-hoc face list misclassifies narrow gaps, shared surfaces, or regions whose interior contains no face. Downstream stages — portal generation, exterior leaf culling, portal vis — consume solidity as authoritative.

### Brush role spectrum

A worldspawn brush in this engine family can carry one of several roles, each with different tradeoffs between BSP participation and runtime mutability. The compiler currently implements only the first row; the others are forward affordances and are listed here so the BSP's constraints are not mistaken for whole-engine constraints.

| Role | BSP splitter? | Solidity contributor? | Runtime mutability | Storage |
|------|---------------|-----------------------|--------------------|---------|
| Splitter brush | yes | yes (structural) | none — moving one invalidates the splitter set | BSP nodes + leaf face-index lists |
| Detail / leaf brush | no | yes (per-leaf reference) | possible — planes never enter the tree | Per-leaf brush list (not yet implemented) |
| Brush model | no | self-contained mini-hierarchy | full — entity-attached, transformable | Mini-BSP or BVH per entity (entity brushes only today) |
| Runtime instance | no | runtime collision query | full — spawned or streamed at gameplay time | Runtime spatial structure (future) |

The compile-time BSP constrains the *splitter set*, not every brush in the world. A future detail-brush or leaf-brush class would let level authors mark geometry as "renderable and collidable but not load-bearing for vis," skipping BSP construction entirely while keeping the compile-time portal graph intact. Dynamic gameplay objects (doors, lifts, breakable walls) would attach to entities and live outside worldspawn, in line with the Quake/idTech mover convention.

The dedup rule in §brush-side projection only ever fires on splitter brushes — by definition the most-constrained category. Coplanar overlap inside the splitter set is treated as an authoring error worth surfacing, because the right home for "small detail flush against a larger surface" is the future detail-brush class, not a splitter-set tiebreaker.

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

Per-chain depth-first. For each portal visited:

1. **Clip the portal polygon against the current frustum** using Sutherland-Hodgman. An empty clip output (fewer than 3 vertices after clipping) is the unified rejection signal — the portal is entirely outside the current sight cone.
2. **Narrow the frustum through the clipped polygon.** The new frustum is built from the portal plane (near), one edge plane per clipped edge through the camera position, and the far plane carried from the current frustum.
3. **Recurse into the neighbor leaf** with the narrowed frustum. Solid leaves block traversal.

**Per-chain tracking.** Cycle prevention keys on portals crossed in the current chain, not on leaves reached globally. Keying on leaves would drop any chain after the first to arrive at a leaf, losing whichever carried the widest sub-frustum. The visible bitset is the union across chains.

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

- Runtime level compilation
- WAD file support
- Runtime lightmap baking
