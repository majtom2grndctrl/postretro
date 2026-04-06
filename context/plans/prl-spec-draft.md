# Postretro Level Compiler — Technical Specification Draft

## Philosophy

The Postretro level compiler follows a single guiding principle: **solve expensive problems at build time so the player's machine focuses more on cheap work at runtime.** Every feature in this spec exists because either (a) it would cost meaningful per-frame computation if done at runtime, or (b) it makes the compiled level self-describing enough that the engine needs no secondary data files or string parsing at load time.

The compiler takes a `.map` file authored in TrenchBroom and produces a `.prl` binary level file. The engine loads this file and is immediately ready to render, simulate, and play — no shader compilation, no spatial structure construction, no asset processing. The level loads in milliseconds.

---

## Input Format

**Source:** TrenchBroom `.map` files (Quake-format brush geometry with entity definitions).

**Entity Definitions:** A custom `.fgd` file defines Postretro-specific entity types, zone annotations, light anchors, surface properties, and spawn data. The level designer works in TrenchBroom with full autocomplete and validation for all custom properties.

**Parsing:** The Rust `shambler` crate reads `.map` files and provides brush geometry as convex hulls with texture references and entity key-value pairs.

---

## Output Format

**File Extension:** `.prl`

**Format:** Custom binary with a header, section table, and typed data sections. Each section is independently loadable and versioned.

### Format Structure

```
Header
  magic: [u8; 4]          — "PRL\0" identification (first 4 bytes)
  version: u16             — format version for backwards compatibility
  section_count: u16       — number of data sections
  
Section Table
  [section_id: u32, offset: u64, size: u64, version: u16] × section_count

Sections (in any order, referenced by offset):
  Geometry
  Cluster Visibility
  Light Anchors & Influence Maps
  Light Probe Grid
  Navigation Mesh
  Audio Propagation
  Texture References
  Collision Data
```

The section-based layout means new features can be added without breaking existing levels. An older engine encountering an unknown section ID skips it. A newer engine encountering a level without an expected section falls back gracefully.

---

## Compilation Pipeline

The compiler runs each stage sequentially. Each stage is a standalone module that reads the output of previous stages and writes its own section data.

```
.map file
  │
  ├─ 1. Parse ──────────── Read brush geometry, entities, textures
  │
  ├─ 2. Classify ───────── Separate world brushes from entity brushes
  │                         (destructible walls, platforms → entities)
  │
  ├─ 3. Build BSP ──────── Construct BSP tree from world geometry
  │
  ├─ 4. Cluster ─────────── Group BSP leaves into visibility clusters
  │
  ├─ 5. Visibility ──────── Compute cluster-to-cluster PVS
  │
  ├─ 6. Collision ───────── Generate collision hulls from world + entity geometry
  │
  ├─ 7. Light Analysis ─── Compute light influence maps and light probe grid
  │
  ├─ 8. Navigation ──────── Generate navigation mesh from walkable surfaces
  │
  ├─ 9. Audio ───────────── Compute inter-zone sound propagation tables
  │
  ├─ 10. Pack ───────────── Write all sections into .prl binary
  │
  └─ output.prl
```

---

## Table Stakes Features

These are the foundational capabilities that any BSP-derived level compiler must provide. They represent solved problems with well-established algorithms.

### 1. Cluster-Based Potentially Visible Set (PVS)

**What it is:** BSP leaves are grouped into clusters (coarse spatial regions). For each cluster, a bitset records which other clusters are potentially visible from it.

**Why it's baked:** Visibility determination is the single most expensive per-frame rendering decision. Without precomputed visibility, the engine would need to test every face or object against occlusion geometry every frame. With a baked PVS, determining visibility is a single array lookup plus a bitset scan — effectively free.

**Why clusters instead of per-leaf:** Coarser granularity means smaller PVS tables (kilobytes instead of hundreds of kilobytes), faster compilation (minutes instead of hours for complex levels), and better tolerance of dynamic changes. When a destructible wall is removed or a platform moves, the cluster-level PVS remains approximately correct because it was already conservatively inclusive. The runtime supplements with frustum culling to tighten the result.

**Algorithm:** Group spatially adjacent leaves into clusters (target 100-300 clusters per level). For each cluster pair, trace sample rays between them through portals to determine mutual visibility. Store as a compressed bitset per cluster.

**Runtime savings:** Eliminates 70-90% of invisible geometry from consideration before any per-face testing. The difference between testing thousands of faces and testing dozens per frame.

**Output:** Cluster definitions (leaf assignments), compressed visibility bitsets per cluster.

The compiler uses a BSP tree internally to derive clusters and portals for PVS computation, but the BSP tree is a compiler implementation detail — it is not serialized into the .prl file. The format stores clusters, not BSP nodes. If a better partitioning algorithm is found later, only the compiler changes.

### 2. Collision Geometry

**What it is:** Simplified convex hull representation of all solid geometry for physics and movement collision testing.

**Why it's baked:** Generating convex hulls from arbitrary brush geometry involves decomposition algorithms that are unnecessary to repeat at runtime. The collision data is derived from the same source geometry as the visual data but simplified for efficient intersection testing.

**Algorithm:** Extract solid brush faces, build convex hulls per brush, store as indexed hull arrays. Entity brushes (destructibles, platforms) get their own collision hulls stored separately from world collision.

**Runtime savings:** Modest — hull generation isn't expensive. The primary benefit is data format convenience and avoiding a separate collision file.

**Output:** World collision hulls, entity collision hulls (indexed by entity ID), collision surface material tags.

### 3. Texture Reference Table

**What it is:** A mapping from face indices to texture asset paths, UV coordinates, and surface flags.

**Why it's baked:** Consolidates all texture metadata into a single indexed lookup rather than requiring string parsing or per-face property resolution at load time.

**Output:** Texture path table, per-face texture index + UV data, surface flags (translucent, sky, no-draw).

---

## Innovations

These features go beyond what traditional BSP compilers provide. Each one front-loads computation that would otherwise happen at runtime, or embeds data that traditional formats require secondary files to express.

### 5. Light Influence Maps

**What it is:** For every light source defined in the level (both static fixtures and dynamic light anchors), the compiler precomputes which faces that light can potentially illuminate. This is stored as a per-light face list with precomputed distance attenuation factors and surface normals relative to the light.

**Why it's baked:** Without influence maps, each frame the engine would need to determine, for every active light, which faces are within range and have line-of-sight. This involves raycasting from the light to every nearby face — potentially hundreds of rays per light per frame. With baked influence maps, the engine iterates a precomputed face list per light and applies the lighting math directly. The expensive spatial query (can this light see this face?) is answered once at compile time.

**How it handles dynamic lights:** Light anchors are positions in the level designated by the level designer where dynamic lights may exist. The compiler traces influence for each anchor position. At runtime, when a neon sign is turned on, the engine looks up that anchor's influence map and applies illumination to the listed faces. When the sign is shot out, the engine stops applying that anchor's contribution. No raycasting, no spatial queries — just a list lookup.

**Runtime savings:** Eliminates per-frame light-to-surface raycasting entirely. For a cyberpunk scene with 20-30 light sources, this saves thousands of ray tests per frame. Scales with light count — the more lights in the scene, the greater the savings.

**Output:** Per-light face index lists with precomputed attenuation, surface normal dot products, and distance values. Light anchor definitions with position, default color, default intensity, and influence radius.

### 6. Light Probe Grid

**What it is:** A regular 3D grid of sample points throughout the level, where each probe stores precomputed ambient/indirect light color and intensity from all directions (typically as spherical harmonics or a simple directional color set).

**Why it's baked:** Dynamic direct lighting (from light anchors and influence maps) handles lights shining directly on surfaces. But without indirect/ambient light, areas not directly lit would be pure black. Computing global illumination at runtime is extremely expensive — it requires tracing light as it bounces between surfaces. Baked light probes capture this bounce lighting at compile time through radiosity or path tracing, then the runtime samples the nearest probes to approximate ambient illumination.

**How it works at runtime:** When rendering a surface, the engine samples the nearest light probes (typically 4-8 via trilinear interpolation) to get an ambient light color, then adds direct lighting contributions from active lights via the influence maps. The result is surfaces that feel naturally lit — shadowed areas have ambient color from nearby walls rather than being pitch black — without any runtime bounce computation.

**Runtime savings:** Makes full global illumination practical without per-frame ray tracing. The probe grid is typically 50-200KB of data that replaces computation that would otherwise require millions of rays per frame.

**Output:** 3D grid positions, probe data (SH coefficients or directional color values), grid bounds and resolution.

### 7. Navigation Mesh

**What it is:** A simplified polygon mesh representing all walkable surfaces in the level, with connectivity data that enables pathfinding algorithms (A*, etc.) to find routes between any two walkable points.

**Why it's baked:** Runtime nav mesh generation involves voxelizing the entire level geometry, identifying walkable surfaces based on slope and clearance, building connected polygon regions, and linking them into a searchable graph. For a complex level this takes hundreds of milliseconds to seconds. Every enemy pathfinding query hits this data structure, potentially multiple times per game turn. The nav mesh is derived entirely from static geometry and never changes (entity obstacles are handled separately at runtime through dynamic obstacle avoidance).

**How it handles dynamic elements:** The nav mesh represents the static world's walkability. Moving platforms and destructible walls are handled through nav mesh links — precomputed connection points that can be enabled or disabled at runtime. When a platform arrives at a stop, the engine activates the nav link connecting the two areas. When a wall is destroyed, the engine activates the nav link through the breach. The core nav mesh never changes; only link states toggle.

**Runtime savings:** Eliminates 200-1000ms of level load time for nav mesh generation. More importantly, provides a pre-optimized spatial structure that makes per-query pathfinding faster than it would be on a runtime-generated mesh, because the compiler can spend arbitrary time optimizing polygon shapes and connectivity.

**Output:** Nav mesh polygons with adjacency data, nav links with activation conditions (entity state references), clearance height data per polygon.

### 8. Audio Propagation Tables

**What it is:** Precomputed data describing how sound travels between zones in the level — which zones can hear which other zones, how much sound is attenuated between them, and what reverb characteristics each zone has.

**Why it's baked:** Runtime sound occlusion requires raycasting or portal traversal from every active sound source to the listener position, testing wall materials for absorption, and computing multipath propagation. In a combat scene with dozens of simultaneous sounds (gunshots, explosions, footsteps, ambient machinery), this is thousands of spatial queries per second. Baked propagation tables reduce each sound's occlusion calculation to a table lookup based on source zone and listener zone.

**What the compiler computes:**
- **Zone-to-zone attenuation:** How much sound is reduced traveling between each pair of zones, accounting for wall materials, portal sizes, and path length.
- **Zone reverb profiles:** Room size estimation, surface material analysis, and reverb time calculation per zone. A small metal corridor has tight reflections; a large open atrium has long reverb.
- **Propagation paths:** The sequence of zones sound travels through to reach from source to listener, enabling the engine to apply appropriate filtering per wall traversed.

**Runtime savings:** Moderate per-event savings that compound with sound density. More importantly, enables *higher quality* audio propagation than would be practical to compute at runtime. Without baking, most games use simple distance-based attenuation and skip occlusion entirely.

**Output:** Zone-to-zone attenuation matrix, per-zone reverb parameters (decay time, early reflection pattern, diffusion), propagation path data.

## Lesser-baked ideas

The following ideas are lower priority and higher ambition. Probably best ignored until we validate the baseline format.

### 11. Precomputed Destruction States

**What it is:** For each destructible element in the level, the compiler precomputes the environmental impact of its destruction — updated cluster visibility, modified nav link states, changed audio propagation, and altered light influence maps.

**Why it's baked:** When a destructible wall is removed at runtime, several precomputed systems are affected. Without precomputed destruction states, the engine would need to recompute visibility, update nav mesh connectivity, recalculate audio propagation, and re-trace light influence — all at the moment of destruction, causing a frame hitch. With baked destruction states, the engine swaps to the precomputed "destroyed" variant of each affected system instantly.

**How it works:** The level designer tags brushes as destructible in TrenchBroom. The compiler builds the level twice — once with the element intact, once with it removed — and stores the delta between the two states. At runtime, destruction triggers a swap of the affected data segments.

**Scope management:** The compiler only recomputes systems in the spatial vicinity of the destruction. A wall destroyed in zone 5 doesn't affect visibility or audio in zone 40. The delta is small and localized.

**Runtime savings:** Eliminates destruction-time frame hitches entirely. The environmental impact of destruction is instant because it was already computed.

**Output:** Per-destructible element: visibility delta (cluster bitset patches), nav link state changes, audio propagation table patches, light influence map patches.

### 12. Moving Element State Variants

**What it is:** For each moving element (platforms, doors, elevators, bridges) with defined stop positions, the compiler precomputes system state at each stop — visibility changes, nav link activation, audio propagation changes, and collision hull positions.

**Why it's baked:** Identical reasoning to destruction states. A platform arriving at a new position changes which areas are connected, which zones can see each other, and how sound propagates. Precomputing these per-stop means the runtime swaps state data when the platform reaches a stop rather than recomputing anything.

**How it works:** The level designer defines path points for each moving element in TrenchBroom. The compiler evaluates the level state at each stop position and stores the system deltas between stops.

**Runtime savings:** Same as destruction states — eliminates computational spikes at the moment of state change.

**Output:** Per-moving element, per-stop: collision hull transform, visibility delta, nav link state changes, audio propagation patches.

---

## Build Pipeline Integration

### Development Workflow

```
Designer edits level in TrenchBroom
  │
  ├─ Saves .map file
  │
  ├─ Runs: prl-build level.map -o level.prl
  │    (target: < 30 seconds for a moderately complex level)
  │
  ├─ Engine hot-reloads level.prl
  │
  └─ Designer tests in-engine immediately
```

### Compilation Performance Targets

| Stage | Target Time | Notes |
|-------|-------------|-------|
| Parse | < 1s | Bounded by .map file size |
| BSP Construction | 1-5s | Depends on brush count |
| Clustering | < 1s | Simple spatial grouping |
| Visibility (PVS) | 5-15s | The bottleneck — cluster count matters |
| Collision | < 1s | Direct hull extraction |
| Light Analysis | 2-10s | Ray tracing for influence maps + probe baking |
| Navigation | 2-5s | Voxelization + region building |
| Audio Propagation | 1-3s | Zone-based, not per-face |
| Packing | < 1s | Binary serialization |
| **Total** | **< 30s** | For a typical single-player level |

### Incremental Compilation (Future)

Track which .map regions changed between edits and recompute only affected sections. This could reduce recompilation to 2-5 seconds for small edits, dramatically improving iteration speed.

---

## Engine Loading Contract

The `.prl` format is designed so that the engine's load sequence is:

1. Read header and section table
2. Memory-map or bulk-read each section
3. Upload geometry to GPU
4. Index cluster visibility, PVS, and influence maps into runtime structures
5. Ready to render

**No processing, no generation, no compilation at load time.** The data is in its final form. The engine's job is to read it and use it.

**Target load time:** < 100ms for a typical level on modern hardware. The level should be playable within the same frame the player presses "start."

---

## Implementation Priorities

### Phase 1: Minimum Viable Compiler
- .map parsing via shambler
- BSP tree construction
- Cluster generation
- Cluster-based PVS
- Collision hulls
- Texture references
- Binary format with section table

*Result: Engine can load and render a navigable, visibility-culled BSP level.*

### Phase 2: Lighting Infrastructure
- Light anchor extraction from entities
- Light influence map computation (ray tracing from each anchor)
- Light probe grid generation (radiosity or simplified bounce)

*Result: Engine has fully dynamic lighting with precomputed spatial acceleration.*

### Phase 3: Gameplay Systems
- Navigation mesh generation
- Nav link definitions for moving elements and destructibles
- Spawn table with typed entity data
- Zone metadata

*Result: Engine can spawn enemies that pathfind, place items, and apply per-zone environmental effects.*

### Phase 4: Advanced Precomputation
- Audio propagation tables
- Destruction state variants
- Moving element state variants
- Incremental compilation

*Result: Fully featured compiler producing rich, self-describing levels with instant environmental response to gameplay events.*
