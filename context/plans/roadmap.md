# Implementation Roadmap

> **Lifecycle:** reviewed and updated at the start of each phase. Deleted when all phases are complete.
> **Purpose:** phased plan from "wgpu window exists" through a playable level. Each phase produces something visible and testable.
> **Related:** `context/lib/index.md`, `context/lib/rendering_pipeline.md`

---

## Phase 1: BSP Loading and Wireframe ✓

- [x] Integrate qbsp crate; load a compiled BSP2 file at startup
- [x] Parse BSP geometry: vertices, edges, faces, models
- [x] Upload vertex data to wgpu buffers
- [x] Render BSP faces as wireframe (no textures, no lighting)
- [x] Minimal free-fly camera (raw winit keyboard/mouse, enough to navigate — replaced by action-mapped input in Phase 2)
- [x] Basic PVS culling: determine camera leaf, decompress PVS, skip non-visible leaves

**Testable outcome:** fly through a BSP level in wireframe, PVS culling visibly reduces draw count. ✓

---

## Phase 1.5: PRL Compiler and Voxel-Based Visibility ✓

- [x] PRL binary format (postretro-level-format crate): header, section table, typed sections
- [x] Level compiler (postretro-level-compiler crate): .map parsing via shambler, spatial partitioning, geometry extraction, PVS, binary output
- [x] Voxel grid: rasterize brush volumes into 3D solid/empty bitmap for spatial queries
- [x] Exterior void sealing: flood-fill from player spawn, mark unreachable empty space as solid
- [x] Spatial grid with voxel-aware cell classification: solid cells skipped, boundary cells subdivided, air cells merged into face-containing clusters
- [x] Ray-cast PVS via 3D-DDA through voxel grid (replaces BSP portal flood-fill)
- [x] Engine PRL loader: file extension dispatch, cluster-based wireframe rendering with per-cluster coloring
- [x] Visibility confidence diagnostics: --diagnostics flag, PRL confidence section, engine gradient rendering
- [x] Test maps: varied-scale rooms (gen_test_map_4.py), contract test suite (107 tests, all passing)

**Testable outcome:** compile .map → .prl, fly through in wireframe with voxel-based PVS culling. Visibility matches expectations across varied room sizes. ✓

**Status note:** PRL compiler works but BSP + portal PVS may replace the voxel pipeline. Voxel code remains in repo. See `context/reference/voxels-vs-bsp-tradeoffs.md` for analysis.

---

## Phase 2: Input and Frame Timing

- [ ] Fixed-timestep frame loop: accumulator, interpolation factor, delta-time clamping
- [ ] Input subsystem: action mapping (keyboard/mouse via winit, gamepad via gilrs)
- [ ] Mouse capture, sensitivity, invert-Y
- [ ] Replace raw free-fly camera with action-driven camera (still no collision)
- [ ] Gamepad support: analog sticks, dead zones, trigger axes

**Testable outcome:** action-driven camera navigating wireframe levels with stable frame timing. Keyboard, mouse, and gamepad all work.

---

## Phase 3: Textured World

- [ ] Load PNG textures at runtime, matched by texture name strings
- [ ] Depth buffer and back-face culling for solid rendering
- [ ] Create render pipeline: base texture with flat uniform lighting (no lightmaps yet)
- [ ] Material derivation from texture name prefixes (table lookup, logged warnings for unknown prefixes)
- [ ] CSG face clipping to eliminate z-fighting from overlapping brushes (PRL path). See `context/reference/csg-face-clipping.md`.

**Testable outcome:** textured level with uniform lighting. Navigate with action-mapped input. No z-fighting.

---

## Phase 4: Light Probes

Validate probe-only surface lighting before committing to lightmaps or a custom compiler. ericw-tools already generates the probe data (`light -lightgrid` → LIGHTGRID_OCTREE lump). This phase answers: does probe-sampled surface lighting look right for the target aesthetic?

- [ ] Parse LIGHTGRID_OCTREE lump from BSP
- [ ] Sample nearest probes for surface lighting (trilinear interpolation between 4–8 probes)
- [ ] Replace flat uniform lighting from Phase 3 with probe-sampled lighting on all surfaces
- [ ] Evaluate visual quality: large surfaces, tight corridors, transitions between bright and dark areas

**Testable outcome:** textured level lit entirely by light probes. Surfaces receive spatially varying illumination from baked probe data. No lightmap atlas, no per-face lightmap UVs.

**Decision gate:** if probe-only lighting looks right, lightmaps may never enter the engine. If it doesn't, fall back to lightmap atlas (RGBLIGHTING lump) in Phase 5. Either way, the experiment cost is one phase.

---

## Phase 5: Lighting Refinement

Direction depends on Phase 4 outcome.

**If probe-only lighting works:**
- [ ] Dynamic point lights (forward pass): muzzle flash, neon signs, explosions — supplementing probe lighting
- [ ] Emissive / fullbright surfaces (neon, screens)
- [ ] Evaluate whether custom probe placement/density justifies a custom compiler stage

**If probe-only lighting falls short:**
- [ ] Build lightmap atlas from RGBLIGHTING lump
- [ ] Two-texture render pipeline: base texture + lightmap
- [ ] Colored lightmaps (RGBLIGHTING)
- [ ] Light probes for sprite/entity lighting only (original LIGHTGRID_OCTREE use case)
- [ ] Dynamic point lights supplementing baked lightmaps

Either path:
- [ ] Billboard sprite rendering: camera-facing textured quads, lit by nearest light probe
- [ ] Fog volumes: resolve `env_fog_volume` to spatial regions, per-fragment fog by distance

**Testable outcome:** fully lit level with dynamic lights, billboard sprites, fog zones.

---

## Phase 6: Post-Processing and Polish

- [ ] Post-processing pass: bloom on emissive/bright surfaces
- [ ] Optional CRT/scanline effect (low priority)
- [ ] Cubemap loading and environment-mapped reflections (consume pre-baked cubemaps from `env_cubemap` positions)

**Testable outcome:** bloom on neon surfaces, reflective surfaces. Optional retro CRT filter.

---

## Phase 7: Grounded Player Movement

- [ ] Player entity with position, velocity, bounding volume
- [ ] Brush volume collision: convex hull intersection using brush half-planes (BSP path: BRUSHLIST BSPX lump; PRL path: brush volumes section). See `context/reference/collision-without-bsp.md`.
- [ ] Gravity and ground detection (walkable surface normal threshold)
- [ ] Slide movement along walls
- [ ] Stair step-up
- [ ] Basic jump

**Testable outcome:** player walks through a level with gravity, collides with walls and floors, steps up stairs, jumps.

---

## Phase 8: Entity Framework and Game Loop

- [ ] Entity model: typed collections, entity parsing (BSP entity lump or .map entities), classname resolution
- [ ] Integrate entities with the fixed-timestep loop (established in Phase 2): entity updates run at fixed tick rate, renderer interpolates entity positions
- [ ] Game event system: entities emit events, audio and renderer consume them
- [ ] Basic entity types: doors (brush model open/close), pickups (billboard, collect on touch), triggers (invisible volumes)

**Testable outcome:** walk through a level with opening doors, collectible pickups, trigger zones that fire events.

---

## Future phases (not yet scoped)

- Audio foundation (kira, spatial audio, reverb zones)
- Enemy entities with AI state machines
- Weapons and projectiles
- HUD and UI
- Specific entity type implementations (see `context/plans/drafts/entity-types/`)
- Cubemap bake tool (see `context/plans/drafts/cubemap-bake-tool/`)
- Custom level compiler (justified when ericw-tools can't produce needed baked data — nav mesh, audio propagation, custom probe density, destruction/movement state variants)
