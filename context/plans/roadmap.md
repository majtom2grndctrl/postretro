# Implementation Roadmap

> **Lifecycle:** reviewed and updated at the start of each milestone. Deleted when all milestones are complete.
> **Purpose:** milestone-by-milestone plan from "wgpu window exists" through a moddable, playable game. Each milestone produces something visible and testable.
> **Related:** `context/lib/index.md`, `context/lib/rendering_pipeline.md`

---

## Milestone 1: BSP Loading and Wireframe ✓

- [x] Integrate qbsp crate; load a compiled BSP2 file at startup
- [x] Parse BSP geometry: vertices, edges, faces, models
- [x] Upload vertex data to wgpu buffers
- [x] Render BSP faces as wireframe (no textures, no lighting)
- [x] Minimal free-fly camera (raw winit keyboard/mouse, enough to navigate — replaced by action-mapped input in Milestone 2)
- [x] Basic PVS culling: determine camera leaf, decompress PVS, skip non-visible leaves

**Testable outcome:** fly through a BSP level in wireframe, PVS culling visibly reduces draw count. ✓

---

## Milestone 1.5: PRL Compiler and Voxel-Based Visibility ✓

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

## Milestone 2: Input and Frame Timing ✓

- [x] Fixed-timestep frame loop: accumulator, interpolation factor, delta-time clamping
- [x] Input subsystem: action mapping (keyboard/mouse via winit, gamepad via gilrs)
- [x] Mouse capture, sensitivity, invert-Y
- [x] Replace raw free-fly camera with action-driven camera (still no collision)
- [x] Gamepad support: analog sticks, dead zones, trigger axes

**Testable outcome:** action-driven camera navigating wireframe levels with stable frame timing. Keyboard, mouse, and gamepad all work. ✓

---

## Milestone 3: Textured World ✓

- [x] Load PNG textures at runtime, matched by texture name strings
- [x] Depth buffer and back-face culling for solid rendering
- [x] Create render pipeline: base texture with flat uniform lighting (no lightmaps yet)
- [x] Material derivation from texture name prefixes (table lookup, logged warnings for unknown prefixes)
- [x] CSG face clipping to eliminate z-fighting from overlapping brushes (PRL path).

**Testable outcome:** textured level with uniform lighting. Navigate with action-mapped input. No z-fighting. ✓

---

## Milestone 3.5: Rendering Foundation Extension ✓

Bring the rendering architecture up to the target pipeline (clustered forward+, GPU-driven indirect draws, SH-probe indirect + normal maps) without adding lighting. This milestone lays the geometry, culling, and draw-dispatch plumbing so later milestones can layer lighting on a stable foundation.

- [x] **Vertex format upgrade** — extend `postretro-level-format` Geometry section to carry packed normals and tangents per vertex (octahedral `u16 × 2` each, plus bitangent sign). prl-build generates them during brush-side projection. Engine vertex layout and world shader updated to consume them. Flat ambient stays in place.
- [x] **Per-cell draw chunks** — restructure prl-build output and engine loader so world geometry is grouped into per-portal-cell chunks with explicit AABB and index range. Replaces per-leaf draw batching. Required for compute culling in the next step.
- [x] **GPU-driven indirect draw path** — compute pass consumes the visible cell list (from portal traversal), runs frustum culling per cell, emits `draw_indexed_indirect` commands into a buffer. Main render pass issues a single `multi_draw_indexed_indirect` call. CPU no longer issues per-cell draws.

**Testable outcome:** textured level with flat ambient, navigable, rendering via GPU-driven indirect draws with portal + frustum culling. Same visual result as Milestone 3, different rendering architecture underneath. Frame time well ahead of 60fps vsync target. ✓

**Note:** Milestone 4 (BVH Foundation) supersedes the per-cell chunk spatial structure shipped here. The vertex format and indirect-draw architecture from Milestone 3.5 remain intact.

---

## Milestone 4: BVH Foundation

Replace Milestone 3.5's per-cell chunk compute cull with a global BVH over all static geometry. Ships with visual parity to Milestone 3.5 — flat ambient, no lighting changes — but lays the spatial structure that Milestone 5's SH baker needs. One acceleration structure, two consumers (runtime cull on the GPU, bake-time ray casts on the CPU).

**Sub-plans:** see `context/plans/drafts/bvh-foundation/`.

- [ ] **Compile-time BVH** — `prl-build` builds a global BVH over all static triangles using the `bvh` crate, flattens to a dense node + leaf array, writes a new `Bvh` PRL section. Retires `chunk_grouping.rs`, `CellChunks` section, and the related compiler glue.
- [ ] **Runtime BVH traversal** — engine loads the `Bvh` section into GPU storage buffers, rewrites `compute_cull.rs` as a WGSL BVH traversal compute shader, deletes legacy fallback paths. Preserves Milestone 3.5's fixed-slot indirect buffer and `multi_draw_indexed_indirect` design.
- [ ] **Check-in gate** — visual parity with Milestone 3.5 confirmed by manual screenshot review. Frame time within reasonable bounds. If global BVH underperforms on cell-heavy maps, decide whether to pivot to per-region BVH before Milestone 5 begins.

**Testable outcome:** identical visual output to Milestone 3.5, rendered through a BVH-based spatial structure. Milestone 5 (Lighting Foundation) is unblocked.

**Architectural commitments locked here:**
- Global BVH, not per-region. Per-region is the pivot path if global underperforms.
- Software traversal only — no hardware ray tracing. Pre-RTX hardware target; wgpu doesn't expose hardware RT regardless.
- Portals stay. Portal DFS still produces the visible-cell set; BVH replaces per-chunk frustum culling, not occlusion culling.
- No backward compat. Pre-release — own the refactor.

---

## Milestone 5: Lighting Foundation

Replace flat ambient with the full target lighting pipeline: SH irradiance volume for indirect, clustered forward+ dynamic lights for direct, normal maps for surface detail, shadow maps for dynamic lights. Milestone 5 delivers a fully lit level. The architectural direction is locked in `context/lib/rendering_pipeline.md` §4.

**Prerequisite:** Milestone 4 (BVH Foundation). The SH baker ray-casts through the BVH built in Milestone 4 — one structure, two consumers, no second design pass.

**Sub-plans:** see `context/plans/drafts/lighting-foundation/`.

- [ ] **FGD light entities** — define `light`, `light_spot`, `light_sun` in `assets/postretro.fgd`. Parser extracts property bags; translator converts to canonical format; validation blocks compilation on errors.
- [ ] **Canonical light format** — `CanonicalLight` struct in the compiler, format-agnostic, fed by per-format translators (`format/quake_map.rs` first; future `format/udmf.rs` etc.).
- [ ] **SH irradiance volume baker** — prl-build stage that places probes on a regular 3D grid over empty space, evaluates SH L2 coefficients by raycasting against static geometry through the Milestone 4 BVH with canonical lights as sources, and writes a new PRL section. Probe validity mask flags probes inside solid brushes.
- [ ] **Runtime SH probe sampling** — parse the probe section into a 3D texture, sample trilinearly in the world shader, replace flat ambient with the SH-reconstructed irradiance.
- [ ] **Normal map rendering** — author normal maps alongside albedo in `textures/`, load them as BC5 (or RGBA placeholder), reconstruct TBN in vertex shader, perturb per-fragment normal before shading.
- [ ] **Clustered forward+ direct lighting** — compute prepass builds per-cluster light index lists from canonical lights plus transient gameplay lights. World shader walks its cluster and accumulates direct contributions.
- [ ] **Shadow maps for dynamic lights** — cascaded shadow maps for directional lights, cube shadow maps for point and spot lights. Low-resolution, nearest-neighbor sampling — chunky pixel shadow edges match the target aesthetic.
- [ ] **Lighting test maps** — author maps that exercise indirect bleed, direct falloff, bright-to-dark transitions, normal-mapped surfaces at varied angles.

**Testable outcome:** textured, normal-mapped level with spatially varying indirect illumination from baked SH probes, dynamic point/spot/directional lights casting shadow-mapped shadows. FGD light entities author both the bake inputs and the runtime direct lights from one source.

**Shadow coverage:** the SH irradiance volume captures indirect light bounces at bake time; dynamic shadow maps cover direct-light occlusion at runtime. Together these replace what lightmaps would contribute in a traditional Quake-lineage pipeline.

---

## Milestone 6: Embedded Scripting and Entity Foundation

Make modding a first-class concern by building the entity model and the scripting layer together from day one. This milestone is bigger than "pick a scripting language and add bindings" — it defines the entity API surface that every subsequent milestone (player movement, weapons, NPCs, world entities) consumes through scripts rather than through hardcoded Rust.

- [ ] Choose embedded scripting language (Rhai, Lua via mlua, or similar — research and decision in a draft plan)
- [ ] Entity model: typed collections, lifecycle (spawn / update / destroy), parent/child relationships, world-space transforms
- [ ] Entity parsing from `.map` entity lump → typed entities at compile time, classname-keyed
- [ ] Fixed-timestep integration: entity updates run at the fixed tick rate established in Milestone 2; renderer interpolates entity positions
- [ ] Game event system: entities emit events, scripts subscribe, audio and renderer consume
- [ ] Script bindings for the entity API: spawn / query / move / event subscribe / event emit
- [ ] Hot reload of scripts during development
- [ ] Documentation: modder-facing API reference

**Testable outcome:** a level loads, entities defined in `.map` files spawn into the world, simple scripted behaviors run (e.g., a door that opens when touched, written entirely in script).

**Why scripting first:** retrofitting a scripting layer onto a Rust-native entity system is far harder than building both at once. By making this Milestone 6, every subsequent feature (player movement, weapons, NPCs) gets designed with the modder API as a primary surface, not as an afterthought.

---

## Milestone 7: Player Movement (Modder-Friendly)

Player movement split into two layers: an engine floor (collision, raycasts, ground detection) and a script API that lets modders craft their own movement style — Quake-style bunnyhop, Doom-style sliding, Half-Life-style air control, whatever the modder wants.

- [ ] Engine floor: brush volume collision via convex hull intersection (BSP path: BRUSHLIST BSPX lump; PRL path: brush volumes section). See `context/reference/collision-without-bsp.md`.
- [ ] Engine floor: ground detection (walkable surface normal threshold), raycasts, sweep tests
- [ ] Script API: expose collision/raycast primitives, expose player input state, expose entity transforms
- [ ] Reference movement script: a default first-person movement implementation written entirely in script, demonstrating gravity, walls, stair step-up, jump
- [ ] Hot reload the movement script during gameplay

**Testable outcome:** player walks through a level with gravity, collides with walls and floors, steps up stairs, jumps — and a modder can swap the movement script for their own without touching engine code.

---

## Milestone 8: Weapons (Modder-Friendly)

Weapons as scripted entities. Engine provides projectile primitives, hit detection, and visual/audio hooks; script defines weapon behavior.

- [ ] Engine primitives: projectile spawning, hitscan raycasts, damage events
- [ ] Script API: weapon definition (fire rate, ammo, projectile type, damage), pickup behavior, viewmodel hooks
- [ ] Reference weapons: a couple of examples covering hitscan and projectile modes

**Testable outcome:** scripted weapons fire, do damage, and can be added or modified by editing scripts.

---

## Milestone 9: NPC Entities (Modder-Friendly)

NPCs as scripted entities with engine-provided AI primitives.

- [ ] Engine primitives: navigation queries, line-of-sight tests, animation hooks
- [ ] Script API: AI state machines, perception (sight/sound), behavior trees or coroutines
- [ ] Reference NPC: a basic enemy with patrol / chase / attack behavior, written in script

**Testable outcome:** scripted NPCs spawn from `.map` entities, navigate the world, react to the player.

---

## Milestone 10: World Entities (Scripted)

Doors, pickups, triggers, monster closets, scripted set pieces — all authored as scripted entities. The bar is "what if monster closets could be scripted to feel more badass."

- [ ] Common base entity types in script: door, pickup, trigger volume, brush mover
- [ ] Script API: brush model manipulation, sound triggers, visual effects, timeline/sequence helpers
- [ ] Sample set pieces: a scripted ambush, a moving platform, a scripted door sequence

**Testable outcome:** a level walkthrough with scripted doors, pickups, ambush triggers, and a set piece — all modifiable by editing scripts.

---

## Future / unscoped

Features and milestones that aren't on the critical path but will likely come up:

- **Visual polish** — billboard sprite rendering, emissive / fullbright surfaces (neon, screens), fog volumes (`env_fog_volume`)
- **Post-processing** — bloom on emissive surfaces, optional CRT/scanline filter, environment-mapped reflections from baked cubemaps (`env_cubemap`)
- **Audio foundation** — kira integration, spatial audio, reverb zones, weapon and footstep sounds
- **HUD and UI** — health, ammo, crosshair, menus
- **Cubemap bake tool** — see `context/plans/drafts/cubemap-bake-tool/`
- **Custom level compiler** — justified when ericw-tools can't produce needed baked data (nav mesh, audio propagation, custom probe density, light influence maps, destruction/movement state variants)
- **Specific entity type libraries** — see `context/plans/drafts/entity-types/`
- **Multi-format map support** — UDMF, etc., via `format/<name>.rs` sibling modules
