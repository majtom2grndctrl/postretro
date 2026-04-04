# Implementation Roadmap

> **Lifecycle:** reviewed and updated at the start of each phase. Deleted when all phases are complete.
> **Purpose:** phased plan from "wgpu window exists" through a playable level. Each phase produces something visible and testable.
> **Related:** `context/lib/index.md`, `context/lib/rendering_pipeline.md`

---

## Phase 1: BSP Loading and Wireframe (current starting point: wgpu window exists)

- [ ] Integrate qbsp crate; load a compiled BSP2 file at startup
- [ ] Parse BSP geometry: vertices, edges, faces, models
- [ ] Upload vertex data to wgpu buffers
- [ ] Render BSP faces as wireframe (no textures, no lighting)
- [ ] Minimal free-fly camera (raw winit keyboard/mouse, enough to navigate — replaced by action-mapped input in Phase 2)
- [ ] Basic PVS culling: determine camera leaf, decompress PVS, skip non-visible leaves

**Bootstrap:** create a simple test map in TrenchBroom using the Postretro game config — a few rooms connected by corridors, enough to verify BSP loading and PVS culling. Compile with `qbsp -bsp2 -notex -wrbrushes` and `vis`. No lighting needed for wireframe. This test map serves all phases as a development fixture.

**Testable outcome:** fly through a BSP level in wireframe, PVS culling visibly reduces draw count.

---

## Phase 2: Input and Frame Timing

- [ ] Fixed-timestep frame loop: accumulator, interpolation factor, delta-time clamping
- [ ] Input subsystem: action mapping (keyboard/mouse via winit, gamepad via gilrs)
- [ ] Mouse capture, sensitivity, invert-Y
- [ ] Replace raw free-fly camera with action-driven camera (still no collision)
- [ ] Gamepad support: analog sticks, dead zones, trigger axes

**Testable outcome:** action-driven camera navigating the wireframe BSP with stable frame timing. Keyboard, mouse, and gamepad all work. Input and wireframe verify each other.

---

## Phase 3: Textured World

- [ ] Load PNG textures at runtime, matched by BSP texture name strings
- [ ] Build lightmap atlas from BSP lightmap data (monochrome LIGHTING lump)
- [ ] Create render pipeline: base texture + lightmap, two-texture sampling per face
- [ ] BSPX colored lightmaps (RGBLIGHTING): upgrade atlas to RGB if lump present
- [ ] Depth buffer and back-face culling for solid rendering
- [ ] Material derivation from texture name prefixes (table lookup, logged warnings for unknown prefixes)

**Testable outcome:** textured, lit BSP level. Navigate with action-mapped input. Correct lighting, no z-fighting.

---

## Phase 4: Audio Foundation

- [ ] Audio subsystem: kira integration, basic sound playback
- [ ] Spatial audio: 3D positional sounds, distance attenuation
- [ ] Reverb zones: resolve `env_reverb_zone` to BSP leaves, apply per-leaf reverb
- [ ] Test sounds tied to camera movement or manual triggers for verification

**Testable outcome:** spatial sounds in the lit level, reverb changes as camera enters different zones.

---

## Phase 5: Advanced Lighting and Sprites

- [ ] BSPX directional lightmaps (LIGHTINGDIR): per-pixel specular term (Blinn-Phong approximation)
- [ ] Emissive / fullbright surfaces (neon, screens)
- [ ] Billboard sprite rendering: camera-facing textured quads
- [ ] BSPX light grid (LIGHTGRID_OCTREE): sample probes to light sprites — verify experimental Q1 BSP2 support; implement fallback (nearest-lightmap or ambient + nearest-light) if grid unavailable
- [ ] Dynamic point lights (forward pass): muzzle flash, neon signs — small count, supplementing baked lighting
- [ ] Fog volumes: resolve `env_fog_volume` brush entities to BSP leaves, apply per-leaf fog in fragment shader

**Testable outcome:** fully lit level with specular highlights, billboard sprites lit by light probes, dynamic lights, fog zones.

---

## Phase 6: Post-Processing and Polish

- [ ] Post-processing pass: bloom on emissive/bright surfaces
- [ ] Optional CRT/scanline effect (low priority)
- [ ] Cubemap loading and environment-mapped reflections (consume pre-baked cubemaps from `env_cubemap` positions)

**Testable outcome:** bloom on neon surfaces, reflective surfaces. Optional retro CRT filter.

---

## Phase 7: Grounded Player Movement

- [ ] Player entity with position, velocity, bounding volume
- [ ] BSP world collision (BRUSHLIST BSPX lump for convex hull collision; fall back to clipnode hulls if unavailable)
- [ ] Gravity and ground detection (walkable surface normal threshold)
- [ ] Slide movement along walls
- [ ] Stair step-up
- [ ] Basic jump

**Testable outcome:** player walks through a BSP level with gravity, collides with walls and floors, steps up stairs, jumps.

---

## Phase 8: Entity Framework and Game Loop

- [ ] Entity model: typed collections, BSP entity lump parsing, classname resolution
- [ ] Integrate entities with the fixed-timestep loop (established in Phase 2): entity updates run at fixed tick rate, renderer interpolates entity positions
- [ ] Game event system: entities emit events, audio and renderer consume them
- [ ] Basic entity types: doors (brush model open/close), pickups (billboard, collect on touch), triggers (invisible volumes)

**Testable outcome:** walk through a level with opening doors, collectible pickups, trigger zones that fire events.

---

## Future phases (not yet scoped)

- Enemy entities with AI state machines
- Weapons and projectiles
- HUD and UI
- Specific entity type implementations (see `context/plans/drafts/entity-types/`)
- Cubemap bake tool (see `context/plans/drafts/cubemap-bake-tool/`)
