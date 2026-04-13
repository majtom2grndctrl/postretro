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

**PRL path:** prl-build builds a BSP tree as a compiler intermediate, generates portal geometry and per-cell draw chunks, and packs runtime data into a custom binary format. The BSP tree drives spatial partitioning and portal generation at compile time; the runtime consumes cells, portals, and chunk tables without walking BSP nodes. (`BspNodes` and `BspLeaves` sections are still emitted for camera-leaf lookup; replacing that with a cell-location section is a future step.) Default mode stores portal geometry for runtime traversal; `--pvs` mode computes a precomputed PVS instead. Engine loads via the postretro-level-format crate.

---

## Supported Map Formats

prl-build accepts idTech2 `.map` files (Quake 1/2 dialect, parsed via shambler/shalrath). Unit scale: 1 unit = 0.0254 m (one inch, exact).

**Texture projection:** both Standard (axis-aligned) and Valve 220 (explicit UV axes) are supported end-to-end. Shalrath auto-detects per face, so they can coexist in one `.map` file — TrenchBroom produces mixed output. UV computation handles both variants.

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

## PRL Compilation

### Compiler pipeline

```
parse .map → brush-volume BSP construction → brush-side projection → portal generation → exterior leaf culling → portal vis → geometry → pack .prl
```

1. **Parse.** Shambler extracts brush volumes, brush sides, and entities. Parse applies two transforms at the boundary: axis swizzle (Quake Z-up → engine Y-up) and unit scale (idTech2: 0.0254 m/unit, exact). Vertex positions, entity origins, and plane distances convert to engine meters; plane normals receive the swizzle only — scale must not apply to direction vectors. The scale comes from a single map-format source, never duplicated at call sites. Brush sides — the textured half-plane polygons bounding each brush — are grouped per brush; they are the input to BSP construction, not world faces. Light entities route to the translation layer (see §Custom FGD) for validation and canonical-format conversion; they do not participate in BSP construction and feed the Phase 4 SH baker plus the runtime direct-lighting path.
2. **Brush-volume BSP construction.** Partitions space by recursively splitting the world AABB with brush-derived planes. Recursion tracks the inside set — the brush indices whose half-spaces fully contain the current region — and terminates at a leaf when the region is uniformly inside one brush set (solid) or uniformly outside every brush (empty). Leaf solidity is structural: it is established during construction, not inferred from face positions afterward. Splitter candidates are drawn from the full set of bounding planes of candidate brushes, including planes no face sits on, so narrow air gaps and adjacent brush boundaries are always detected. The world AABB is the union of brush AABBs with a one-meter slack margin on each axis. Recursion depth is hard-capped; pathological input yields a compiler error rather than a stack overflow.
3. **Brush-side projection.** Derives world faces from brush sides in two passes. Pass 1 walks each brush side down the tree using plane-index equality as the routing primitive (a polygon on a splitting plane goes to one side only, never both), splits on all other planes, and accumulates surviving fragments into empty leaves as a per-side visible hull. Pass 2 distributes each visible hull back through the tree, emitting a triangulated face in every empty leaf it reaches. Fragments that land in solid leaves are dropped — face-in-solid culling falls out of the walk for free. When two coplanar brush sides reach the same leaf, containment resolves: the fully-contained polygon is dropped as redundant, or if the incoming polygon contains an existing face the existing face is superseded. Partial overlap emits both polygons and leaves any z-fighting as an authoring diagnostic — the compiler does not attempt 2D polygon union, because the intended home for flush decorative detail is a future non-splitting detail-brush class, not a splitter-set tiebreaker. Mismatched textures on a containment drop are surfaced as a warning.
4. **Portal generation.** For each BSP internal node, clips the splitting-plane polygon against ancestor splitting planes to produce the portal polygon bounding that node's partition. Each portal is a convex polygon connecting two adjacent empty leaves. In default mode, portals are stored in the `.prl` file (section 15) for runtime traversal. In `--pvs` mode, portals are used as intermediate data and discarded.
5. **Exterior leaf culling.** Flood-fills through the portal graph from a point outside the map's bounding volume. Every empty leaf reachable from outside is an exterior leaf. Exterior leaves produce no packed geometry — void-facing surfaces of the sealing brushes are absent from the output. A map with a leak has interior leaves incorrectly classified as exterior.
6. **Portal vis** (`--pvs` mode only). Per empty leaf, floods through the portal graph. A leaf L' is potentially visible from L if any sequence of portals connects them. Output: per-leaf PVS bitsets, RLE-compressed. Computed in parallel (one task per leaf).
7. **Geometry.** Fan-triangulates faces into vertex/index buffers. Faces grouped by leaf index for efficient per-leaf draw calls.
8. **Pack.** Writes BSP tree nodes, BSP leaves (face ranges, bounds), and geometry to the `.prl` binary format. Default mode also writes the Portals section (15). `--pvs` mode writes the LeafPvs section (14) instead.

### Leaf solidity

Step 2's inside-set tracking means leaf solidity is known the moment the leaf is produced: inside the intersection of its bounding brushes' half-spaces (solid) or outside all of them (empty). This avoids the class of bugs where centroid-based classification on a post-hoc face list misclassifies narrow gaps, shared surfaces, or regions whose interior contains no face. Downstream stages — portal generation, exterior leaf culling, portal vis — consume solidity as authoritative.

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

Two paths, selected by which PRL section is present:

| PRL section present | Runtime path | Notes |
|---------------------|--------------|-------|
| Portals (15) | Per-frame portal flood-fill with frustum narrowing | Default. Handles corners and narrow apertures without precomputation. |
| LeafPvs (14) | Precomputed PVS bitset lookup | Fallback for `--pvs` builds. |

**Architectural stance: id Tech 4 (Doom 3, 2004), not Quake 1.** Visibility is computed per frame from portal geometry, not baked into a precomputed bitset. Carmack's reasoning for the break from Quake's vis pipeline still applies: precomputed PVS lengthens compile cycles, fights with dynamic geometry, and the per-frame cost is trivial at modern leaf counts. The compiler still supports `--pvs` so a precomputed fallback exists, but the default path is runtime traversal.

Algorithm details (clip-and-narrow, per-chain cycle tracking, clipping robustness): `rendering_pipeline.md` §2.

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
