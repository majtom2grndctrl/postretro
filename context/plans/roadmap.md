# Implementation Roadmap

> **Lifecycle:** reviewed and updated at the start of each milestone. Deleted when all milestones are complete.
> **Purpose:** milestone-by-milestone plan from "wgpu window exists" through a moddable, playable game. Each milestone produces something visible and testable.
> **Related:** `context/lib/index.md`, `context/lib/rendering_pipeline.md`

> **Architectural pivot (Milestone 5 onward):** the engine is being re-cut around **chunks as a universal primitive** — every piece of world geometry is a chunk with a transform, a collider, an SDF contribution, and a dependency link to neighbors. "Static world vs. entity" is gone as a split in the renderer and collision system; identity-transform chunks just happen to stay asleep. Lighting is **probe grid + SDF-based soft shadows**, with CSM retained only as a sun optimization. Cube shadow maps, lightmaps, and the BSP runtime partition are dropped. BSP remains available as a compile-time intermediate only where useful. This pivot front-loads harder graphics work (SDF baking, sphere tracing, probe lighting) in exchange for a simpler runtime and a game that can deliver the "boom" in boomer shooter — floating barges, collapsing floors, and destructible pillars are first-class, not bolted on.

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

**Status note:** superseded by the BVH + portal pipeline in Milestone 4. Voxel code remains in repo as reference.

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

- [x] **Vertex format upgrade** — packed normals and tangents per vertex (octahedral `u16 × 2` each, plus bitangent sign).
- [x] **Per-cell draw chunks** — world geometry grouped into per-portal-cell chunks with explicit AABB and index range.
- [x] **GPU-driven indirect draw path** — compute cull → `multi_draw_indexed_indirect`.

**Testable outcome:** textured level with flat ambient, navigable, rendering via GPU-driven indirect draws with portal + frustum culling. ✓

---

## Milestone 4: BVH Foundation ✓

- [x] **Compile-time BVH** — global SAH BVH over all static triangles, flattened to dense node/leaf arrays in DFS order, new `Bvh` PRL section.
- [x] **Runtime BVH traversal** — WGSL skip-index DFS traversal with visible-cell bitmask fed by portal DFS.
- [x] **Check-in gate** — visual parity with Milestone 3.5 confirmed.

**Testable outcome:** ✓ identical visual output to Milestone 3.5, rendered through a global BVH. Milestone 5 unblocked.

**Durable decisions migrated to `context/lib/`:**
- Global vs. per-region rationale → `rendering_pipeline.md` §5
- `Bvh` PRL section layout → `rendering_pipeline.md` §5 + `build_pipeline.md`
- WGSL skip-index traversal pattern → `rendering_pipeline.md` §7.1

---

## Milestone 5: Lighting Foundation (Probes + SDF)

Replace flat ambient with the full pivot lighting pipeline: a **probe grid** for indirect (SH L2 irradiance volume), **CSM** for directional/sun shadows, and **SDF sphere-tracing** for point and spot shadows. Lightmaps are not baked. Cube shadow maps are not implemented. Lights are authored as `_bake_only` (probe-grid-only contribution) or runtime-dynamic via a single FGD property.

**Prerequisite:** Milestone 4 (BVH). One acceleration structure, three consumers: runtime cull (GPU), SH baker (CPU), SDF baker (CPU).

**Sub-plans:** see `context/plans/in-progress/lighting-foundation/`.

- [x] **FGD light entities** (sub-plan 1) — `light`, `light_spot`, `light_sun` in `assets/postretro.fgd`; canonical light format; `_bake_only` property distinguishes runtime-dynamic lights from probe-grid-only contributors.
- [x] **SH irradiance volume baker** (sub-plan 2) — prl-build stage; ray-casts through the Milestone 4 BVH; SH L2 projection; validity mask.
- [x] **Direct lighting loop** (sub-plan 3) — flat per-fragment light loop over runtime lights; per-type evaluation; Lambert diffuse.
- [x] **Light influence volumes** (sub-plan 4) — per-light sphere bounds in PRL; runtime spatial culling; gates CSM slot assignment and SDF sphere-trace per-light activation.
- [x] **CSM sun shadows** (sub-plan 5) — 3 cascades, 1024², bounding-sphere fit with rotation-invariant texel snapping. Hard edges match aesthetic; SDF path provides penumbrae elsewhere.
- [x] **Runtime probe sampling** (sub-plan 6) — parse SH section as 3D texture; trilinear sample in world shader for both static surfaces and dynamic entities.
- [x] **Animated SH layers** (sub-plan 7) — per-light monochrome SH layers, animation descriptor + sample buffers, per-frame brightness/color curve evaluation in the fragment shader.
- [ ] **SDF atlas + sphere-traced soft shadows** (sub-plan 8) — brick-indexed sparse distance field baked by prl-build; WGSL sphere trace per visible shadow-casting point/spot light; soft penumbrae from spot cone angle. Chunk-friendly brick addressing so Milestone 8's migration is additive. Target: 1–2 ms total across all visible lights.
- [ ] **Specular maps** (sub-plan 9) — per-texel specular highlights in the direct light loop. Shading model decision (Phong vs. PBR) required before implementation starts.
- [ ] **Lighting test maps** — exercise probe indirect, CSM sun shadows, SDF soft shadows for point/spot, specular surfaces.

**Testable outcome:** textured level with probe-sampled indirect on both static surfaces and dynamic entities, CSM-driven sun shadows, SDF-driven soft shadows for point and spot lights, and per-texel specular highlights. Static lights contribute only to the probe bake; dynamic lights participate in both the bake and the runtime direct loop.

---

## Milestone 6: Sector Graph + Portal Culling

Replace the BSP-as-runtime-scaffolding with an **author-defined sector graph**. BSP stays as an optional compile-time intermediate for convex cell decomposition only where useful; the runtime no longer walks BSP nodes. This unblocks kinematic clusters (they need their own sector graphs) and destruction (latent portals).

- [ ] **Sector volume authoring** — FGD entity or brush tag for author-defined sector volumes in `.map`.
- [ ] **Sector graph extractor** — prl-build emits a sector graph with portal polygons on edges. Replaces `BspNodes`/`BspLeaves` sections for runtime use.
- [ ] **Latent portals** — portals flagged as "activate on event" (for pre-authored destruction reveals).
- [ ] **Runtime portal PVS** — hierarchical frustum-through-portal culling over the sector graph; feeds the BVH cull shader's visible-cell bitmask.
- [ ] **Retire BSP runtime path** — remove camera-leaf lookup, `BspNodes`/`BspLeaves` section parsing; BSP compiler stages that aren't consumed can stay or be removed as convenient.

**Testable outcome:** identical visual + perf to Milestone 5, with sector graph replacing BSP as the visibility authority.

---

## Milestone 7: Entity Model (Rust-only)

Establish the core entity layer in pure Rust before any scripting language is bound to it. The goal is a fast-iteration window: rename fields, restructure event types, change lifecycle shapes — all with Rust's compiler catching inconsistencies, no scripting contract to break. The constraint that shapes every decision here is **scripting-layer exposure**: all public APIs must be expressible in a scripting language's terms — IDs/handles rather than Rust references, simple owned event types, no lifetimes in the surface, no generics that can't be erased at the binding layer.

- [ ] **Typed entity collections** — spawn / query / destroy, keyed by stable numeric ID. Classname registry for FGD-defined types.
- [ ] **Lifecycle** — spawn, update tick, destroy. Parent/child relationships with transform inheritance. Updates run in the fixed-timestep game logic stage.
- [ ] **World-space transforms** — position, rotation, scale. Interpolation state for the render stage.
- [ ] **Event system** — entities emit typed events; subscriptions are classname- or ID-scoped. Event types are simple owned structs (scripting-bindable by construction).
- [ ] **Scripting surface audit** — before moving on, review the entire public API against the constraint: "could a Lua or Rhai script call this?" Flag and fix anything that leaks Rust-specific types. This is the cheap window to make those changes.
- [ ] **Reference entity behaviors (Rust-only)** — a `RotatorDriver` (sets a target transform each tick), a `DamageSource` (debug keybind → emits a damage event). These are the first consumers that validate the API shape.

**Testable outcome:** spawn a `RotatorDriver` entity in a test level, confirm it emits transform events at the fixed tick rate. Nothing visual yet — this is architecture validation. The public API surface passes the scripting-exposure audit.

**Why before scripting:** once a scripting runtime is bound, every API shape decision becomes a public contract. Iterate fast in Rust first, stabilize, then bind. The `DamageSource` and `RotatorDriver` reference behaviors also directly feed Milestones 8 and 9 as their stub drivers.

---

## Milestone 8: Chunk Primitive + Physics

Promote the renderer's unit-of-work to a **chunk**: mesh + collider + SDF contribution + transform + dependency link. Static world geometry is "chunks with identity transforms, asleep in the broadphase." Stand up a rigidbody solver so kinematic and dynamic chunks have somewhere to live.

**Note:** "chunk" supersedes the "per-portal-cell draw chunk" term from Milestone 3.5. The old draw chunks are renamed to "draw ranges" in the PRL schema. One term, one concept going forward.

- [ ] **Chunk record in PRL** — replace the current static-geometry schema: each chunk carries its mesh range, collider hull, SDF brick refs, sector membership, and neighbor dependency edges. Identity-transform by default. (Rename "draw chunk" → "draw range" in existing code and docs.)
- [ ] **Unified chunk render path** — one draw path for all geometry; renderer iterates the chunk pool each frame.
- [ ] **Chunk collider broadphase** — BVH over chunk AABBs; asleep chunks cached, awake chunks re-inserted each tick.
- [ ] **Rigidbody solver** — select and integrate a Rust physics library (research doc required before implementation; Rapier is the likely candidate). Sleep/wake integration with the chunk pool.
- [ ] **Frame-order update** — physics tick placed between Game logic and Audio. This extends the canonical frame order to: Input → Game logic → **Physics** → Audio → Render → Present. Update `CLAUDE.md` and `index.md` when this lands.

**Testable outcome:** same visual result as Milestone 7, but a test map with a single "free" chunk demonstrates rigidbody motion driven by the solver. World geometry renders through the unified chunk path.

---

## Milestone 9: Kinematic Clusters (Moving Geometry)

A **kinematic cluster** is a sub-world compiled like the main world but with a transform — the barge, the elevator, the tilting floor. Same chunks, same renderer path, same collider path, just a non-identity transform. Cluster transforms are driven by entities from Milestone 7 (`KinematicDriver` entity sets the transform each tick).

- [ ] **Cluster authoring** — FGD entity or brush tag for cluster volumes; compiler emits each cluster as its own sector graph + chunk group.
- [ ] **Dynamic portals at cluster boundaries** — when a cluster's ramp/hatch aligns with a static sector portal, connect them at runtime. V1 scope: portal activates only when cluster transform is within ε of a docked pose.
- [ ] **Cluster transforms in the render path** — chunk draw calls consume the cluster transform; shadow pass includes cluster chunks automatically (CSM and SDF already re-evaluate casters each frame).
- [ ] **Cluster colliders** — broadphase transforms cluster AABBs into world space each tick.
- [ ] **Reference clusters** — a moving elevator and a drifting barge test map. Both driven by `KinematicDriver` entities authored in the `.map` file.

**Testable outcome:** stand on a moving barge, ride an elevator. Lighting and shadows behave correctly. Collision is stable on moving surfaces.

---

## Milestone 10: Destruction (Pre-Fracture + Promotion)

Destruction is topology change expressed at **authoring time**: brushes are pre-fractured into chunks with dependency edges, and runtime just promotes chunks from asleep-static to awake-dynamic when damage or dependency failure says so. Interior break-faces are pre-authored and unhidden on fracture.

**Note on damage source:** weapons are not available until Milestone 13. The "Die Hard" test map uses a `DamageSource` entity (debug keybind → emits damage event) from Milestone 7 as the trigger.

- [ ] **Fracture authoring** — FGD properties for `hp`, `supports=[...]`, and interior break-face tagging; compiler emits dependency graph as a new PRL section.
- [ ] **Promotion pipeline** — damage system walks the dependency graph, moves chunks from static pool to dynamic-chunk pool, hands rigidbodies over, reveals interior faces, activates latent portals (from Milestone 6).
- [ ] **SDF invalidation** — mark affected SDF bricks dirty on promotion; sphere-trace queries run against stale data during the dirty window (visual error accepted for 1–2 frames). Optional partial rebake streamed over frames.
- [ ] **Reference scenarios** — the "Die Hard pillar" test map: trigger the `DamageSource` entity, floor slabs above lose support and collapse.

**Testable outcome:** trigger a keybind, watch a pillar take damage, watch dependent floor sections collapse with correct lighting, collision, and culling.

---

## Milestone 11: Scripting Layer

Bind the entity API from Milestone 7 to an embedded scripting language. By this point the entity model has been validated by two real consumers (kinematic clusters, destruction), so the API surface is stable. The scripting language research doc is a prerequisite — this is a big decision.

- [ ] **Research and language selection** — draft plan comparing candidates (JavaScript via QuickJS, Lua via mlua, Rhai, Wren, etc.) against the scripting-exposure API from Milestone 7. Decide before implementation starts.
- [ ] **Script runtime integration** — embed chosen language; hook into fixed-timestep update and event dispatch.
- [ ] **Bindings** — entity API (spawn / query / move / destroy), event subscribe/emit, chunk promotion, cluster transform control, damage events, portal activation. All bindings must honor the scripting-exposure constraints from Milestone 7.
- [ ] **Entity parsing from `.map`** — `.map` entity lump → typed entities at compile time, classname-keyed.
- [ ] **Hot reload** — reload scripts during gameplay without restarting.
- [ ] **Modder-facing API reference** — generated or hand-written, covers all bound APIs.

**Testable outcome:** a scripted door (kinematic cluster driven by script instead of `KinematicDriver`), a scripted trigger, a scripted destruction sequence — all editable without touching Rust. Replace at least one Milestone 7 Rust-only driver behavior with a script equivalent.

---

## Milestone 12: Player Movement (Modder-Friendly)

- [ ] Engine floor: chunk-collider queries (convex hull intersection against awake + relevant static chunks), ground detection, sweep tests, raycasts.
- [ ] Script API: collision/raycast primitives, input state, entity transforms.
- [ ] Reference movement script: gravity, walls, stair step-up, jump — written entirely in script.
- [ ] Hot reload of movement script during gameplay.

**Testable outcome:** player walks through a level with full collision and movement, including standing on moving clusters and reacting to destruction. Modder can swap the movement script.

---

## Milestone 13: Weapons (Modder-Friendly)

- [ ] Engine primitives: projectile spawning, hitscan raycasts through the chunk collider pool, damage events (feeds Milestone 10's promotion pipeline with real weapons replacing the `DamageSource` debug entity).
- [ ] Script API: weapon definition, pickup behavior, viewmodel hooks.
- [ ] Reference weapons: hitscan + projectile examples, at least one that triggers chunk destruction.

**Testable outcome:** scripted weapons that can knock clusters around and shoot out support pillars.

---

## Milestone 14: NPC Entities (Modder-Friendly)

- [ ] Engine primitives: navigation queries, line-of-sight via SDF (free benefit of the Milestone 5 SDF), animation hooks.
- [ ] Script API: AI state machines, perception, behavior trees or coroutines.
- [ ] Reference NPC: patrol / chase / attack, entirely in script.

**Testable outcome:** scripted NPCs that navigate dynamic worlds — they path around destroyed sections and ride moving clusters.

---

## Milestone 15: World Entities and Set Pieces

Most traditional "world entity" types (doors, movers, ambushes) are already expressible as kinematic clusters + scripts by this point. This milestone fills in what's left.

- [ ] Common base scripts: pickup, trigger volume, timeline/sequence helpers.
- [ ] Visual/audio effects hooks for set pieces.
- [ ] Sample set piece: a scripted ambush that includes destruction choreography.

**Testable outcome:** a level walkthrough with scripted doors, pickups, ambush triggers, and a destruction set piece — all modifiable by editing scripts.

---

## Future / unscoped

- **Visual polish** — billboard sprite rendering, emissive / fullbright surfaces, fog volumes
- **Post-processing** — bloom, optional CRT/scanline filter, baked cubemap reflections
- **Audio foundation** — kira integration, spatial audio, reverb zones
- **HUD and UI** — health, ammo, crosshair, menus
- **Cubemap bake tool** — see `context/plans/drafts/cubemap-bake-tool/`
- **Dynamic SDF rebake for mid-level destruction** — partial brick updates around fracture events, beyond Milestone 9's dirty-marking
- **Specific entity type libraries** — see `context/plans/drafts/entity-types/`
- **Multi-format map support** — UDMF, etc., via `format/<name>.rs` sibling modules
