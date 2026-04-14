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

## Phase 2: Input and Frame Timing ✓

- [x] Fixed-timestep frame loop: accumulator, interpolation factor, delta-time clamping
- [x] Input subsystem: action mapping (keyboard/mouse via winit, gamepad via gilrs)
- [x] Mouse capture, sensitivity, invert-Y
- [x] Replace raw free-fly camera with action-driven camera (still no collision)
- [x] Gamepad support: analog sticks, dead zones, trigger axes

**Testable outcome:** action-driven camera navigating wireframe levels with stable frame timing. Keyboard, mouse, and gamepad all work. ✓

---

## Phase 3: Textured World ✓

- [x] Load PNG textures at runtime, matched by texture name strings
- [x] Depth buffer and back-face culling for solid rendering
- [x] Create render pipeline: base texture with flat uniform lighting (no lightmaps yet)
- [x] Material derivation from texture name prefixes (table lookup, logged warnings for unknown prefixes)
- [x] CSG face clipping to eliminate z-fighting from overlapping brushes (PRL path).

**Testable outcome:** textured level with uniform lighting. Navigate with action-mapped input. No z-fighting. ✓

---

## Phase 3.5: Rendering Foundation Extension ✓

Bring the rendering architecture up to the target pipeline (clustered forward+, GPU-driven indirect draws, SH-probe indirect + normal maps) without adding lighting. This phase lays the geometry, culling, and draw-dispatch plumbing so Phase 4 can layer lighting on a stable foundation.

- [x] **Vertex format upgrade** — extend `postretro-level-format` Geometry section to carry packed normals and tangents per vertex (octahedral `u16 × 2` each, plus bitangent sign). prl-build generates them during brush-side projection. Engine vertex layout and world shader updated to consume them. Flat ambient stays in place.
- [x] **Per-cell draw chunks** — restructure prl-build output and engine loader so world geometry is grouped into per-portal-cell chunks with explicit AABB and index range. Replaces per-leaf draw batching. Required for compute culling in the next step.
- [x] **GPU-driven indirect draw path** — compute pass consumes the visible cell list (from portal traversal), runs frustum culling per cell, emits `draw_indexed_indirect` commands into a buffer. Main render pass issues a single `multi_draw_indexed_indirect` call. CPU no longer issues per-cell draws.

**Testable outcome:** textured level with flat ambient, navigable, rendering via GPU-driven indirect draws with portal + frustum culling. Same visual result as Phase 3, different rendering architecture underneath. Frame time well ahead of 60fps vsync target. ✓

**Phase boundary:** no lighting changes in this phase. The world shader still applies flat ambient — the upgrade to SH sampling, normal maps, and dynamic lights is Phase 4. Keeping lighting out isolates the architectural risk of the indirect draw and cell-chunking changes.

---

## Phase 4: Lighting Foundation

Replace flat ambient with the full target lighting pipeline: SH irradiance volume for indirect, clustered forward+ dynamic lights for direct, normal maps for surface detail, shadow maps for dynamic lights. Phase 4 delivers a fully lit level, not a decision gate — the architectural direction is locked in `context/lib/rendering_pipeline.md` §4.

**Sub-plans:**

- [ ] **FGD light entities** — define `light`, `light_spot`, `light_sun` in `assets/postretro.fgd`. Parser extracts property bags; translator converts to canonical format; validation blocks compilation on errors. Drafted in `plans/drafts/phase-4-baked-lighting/` stages 1–3.
- [ ] **SH irradiance volume baker** — prl-build stage that places probes on a regular 3D grid over empty space, evaluates SH L2 coefficients by raycasting against static geometry with canonical lights as sources, and writes a new PRL section. Probe validity mask flags probes inside solid brushes. Drafted in `plans/drafts/phase-4-baked-lighting/` stages 4–5.
- [ ] **Runtime SH probe sampling** — parse the probe section into a 3D texture, sample trilinearly in the world shader, replace flat ambient with the SH-reconstructed irradiance.
- [ ] **Normal map rendering** — author normal maps alongside albedo in `textures/`, load them as BC5 (or RGBA placeholder), reconstruct TBN in vertex shader, perturb per-fragment normal before shading.
- [ ] **Clustered forward+ direct lighting** — compute prepass builds per-cluster light index lists from canonical lights plus transient gameplay lights. World shader walks its cluster and accumulates direct contributions.
- [ ] **Shadow maps for dynamic lights** — cascaded shadow maps for directional lights, cube shadow maps for point and spot lights. Low-resolution, nearest-neighbor sampling — chunky pixel shadow edges match the target aesthetic.
- [ ] **Lighting test maps** — author maps that exercise indirect bleed, direct falloff, bright-to-dark transitions, normal-mapped surfaces at varied angles. Validates the full stack.

**Testable outcome:** textured, normal-mapped level with spatially varying indirect illumination from baked SH probes, dynamic point/spot/directional lights casting shadow-mapped shadows. FGD light entities author both the bake inputs and the runtime direct lights from one source.

**Shadow coverage:** the SH irradiance volume captures indirect light bounces at bake time; dynamic shadow maps cover direct-light occlusion at runtime. Together these replace what lightmaps would contribute in a traditional Quake-lineage pipeline.

---

## Phase 5: Visual Polish

- [ ] Billboard sprite rendering: camera-facing textured quads, lit by the SH volume plus reaching dynamic lights
- [ ] Emissive / fullbright surfaces (neon, screens): bypass lighting modulation, render at full brightness
- [ ] Fog volumes: resolve `env_fog_volume` to spatial regions, per-fragment fog by distance

**Testable outcome:** lit level with billboard sprites, neon surfaces, fog zones. Covers the visual vocabulary gap between "geometry is lit" and "the level feels inhabited."

---

## Phase 6: Post-Processing and Polish

- [ ] Post-processing pass: bloom on emissive/bright surfaces
- [ ] Optional CRT/scanline effect (low priority -- consider running exclusively on UI elements?)
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
- Custom level compiler (justified when ericw-tools can't produce needed baked data — nav mesh, audio propagation, custom probe density, light influence maps (per-light face lists replacing runtime raycasts), destruction/movement state variants)
