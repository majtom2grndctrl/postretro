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

## Milestone 5: Lighting Foundation ✓

- [x] **FGD light entities** — `light`, `light_spot`, `light_sun` in `assets/postretro.fgd`; canonical light format; `_bake_only` property distinguishes runtime-dynamic lights from probe-grid-only contributors.
- [x] **SH irradiance volume baker** — prl-build stage; ray-casts through the Milestone 4 BVH; SH L2 projection; validity mask.
- [x] **Direct lighting loop** — flat per-fragment light loop over runtime lights; per-type evaluation; Lambert diffuse.
- [x] **Light influence volumes** — per-light sphere bounds in PRL; runtime spatial culling; gates CSM slot assignment and SDF sphere-trace per-light activation.
- [x] **CSM sun shadows** — 3 cascades, 1024², bounding-sphere fit with rotation-invariant texel snapping. Hard edges match aesthetic.
- [x] **Runtime probe sampling** — parse SH section as 3D texture; trilinear sample in world shader for both static surfaces and dynamic entities.
- [x] **Animated SH layers** — per-light monochrome SH layers, animation descriptor + sample buffers, per-frame brightness/color curve evaluation in the fragment shader.
- [x] **Lightmaps** — per-face baked direct lighting; static surfaces sample lightmap atlas; dynamic entities fall back to probe grid.

**Testable outcome:** textured level with probe-sampled indirect, lightmapped static surfaces, CSM-driven sun shadows, and animated light layers. ✓

**Scope note:** SDF sphere-traced soft shadows and specular maps were descoped. See the future section.

---

## Milestone 6: Scripting + Entity Foundation ✓

Establish the entity model and scripting layer together. Scripting and entities are co-designed from the start: the entity API is the scripting API, and most entity behaviors are written as scripts rather than Rust. This avoids the two-pass "Rust-only stabilization then bind" approach — the scripting surface constraint shapes the entity model from day one.

- [x] **Language selection** — dual-runtime approach: QuickJS (rquickjs) for TypeScript/JavaScript, Luau (mlua) for Luau. Both runtimes run side by side; scripts dispatched by extension.
- [x] **Entity model** — typed collections (spawn / query / destroy, stable numeric ID); classname registry for FGD-defined types; lifecycle (spawn, tick, destroy); parent/child relationships with transform inheritance; world-space transforms with interpolation state for the render stage.
- [x] **Event system** — typed owned events; classname- or ID-scoped subscriptions. Event types are scripting-bindable by construction (no Rust-specific types in the surface).
- [x] **Scripting runtime** — both VMs embedded; shared definition + behavior contexts; pre-warmed context pool; primitive registry (one registration installs in both runtimes and all future contexts); pooled-context isolation (QuickJS: `Object.freeze(globalThis)`; Luau: sandbox flag). See `context/lib/scripting.md`.
- [x] **Entity API bindings** — spawn / query / move / destroy; event subscribe/emit. All bindings use IDs/handles rather than Rust references; no lifetimes in the surface.
- [x] **Map entity parsing** — `.map` entity lump → typed entities at compile time, classname-keyed. Entities spawn from map data at level load.
- [x] **Hot reload** — file watcher monitors script directory; changed scripts reload on next frame drain. Debug builds only.
- [x] **Reference behaviors (script)** — `RotatorDriver` and `DamageSource` written as scripts. See `content/tests/scripts/`.
- [x] **Modder-facing API reference** — covers all bound APIs. See `docs/scripting-reference.md`.

**Testable outcome:** spawn a scripted entity from a `.map` file; confirm it ticks and emits events at the fixed tick rate. Hot-reload the script during gameplay. The `DamageSource` debug entity is available for future destruction testing. ✓

---

## Milestone 7: Grounded Movement

Player controller with world collision, gravity, and jumping. The player is an entity from Milestone 6. Movement behavior is a script — the engine exposes collision primitives, the reference movement script implements Quake-style grounded movement.

**Prerequisite:** Milestone 6 (entity model + scripting). Player exists as a typed entity; collision API is exposed to scripts.

- [ ] **Collision dependency** — add parry3d.
- [ ] **World trimesh collider** — at level load, register PRL static geometry as a Parry trimesh collider.
- [ ] **Engine collision primitives** — shape cast (sweep), ray cast, point-in-volume overlap — exposed to the script API.
- [ ] **Reference movement script** — gravity, terminal velocity, wall slide, step-up, jump, strafe — written entirely in script using the collision primitives. Quake-style air control vs. grounded-only acceleration: decide during implementation.
- [ ] **Hot reload** — movement script reloads during gameplay.

**Testable outcome:** player walks through a PRL level with full collision response — no clipping, wall slide, step-up, jump. Modder can edit and hot-reload the movement script.

---

## Future / Speculative

Features below are intended but not yet sequenced. Rough priority ordering within each group.

### Gameplay systems

- **Weapons** — hitscan and projectile primitives, scripted weapon definitions, viewmodel hooks; at least one weapon triggers chunk destruction. Requires grounded player (Milestone 7) and damage events (Milestone 6).
- **NPC Entities** — navigation queries, line-of-sight, scripted AI state machines (patrol / chase / attack). Navigation against the world trimesh collider from Milestone 7.
- **World Entities** — common base scripts for doors, pickups, trigger volumes, timeline/sequence helpers; a scripted ambush set piece with destruction choreography.

### Moving and destructible geometry

- **Kinematic Clusters** — sub-worlds compiled like the main world but with a runtime transform (elevators, barges). Cluster authoring in TrenchBroom, compiler emits per-cluster geometry, `KinematicDriver` entity sets transform each tick. Dynamic portals at cluster boundaries when aligned with a static sector portal.
- **Destruction (Pre-Fracture + Promotion)** — brushes pre-fractured into pieces with dependency edges at compile time. Runtime promotes pieces from static to dynamic on damage; reveals pre-authored interior break-faces. Requires a full rigidbody solver (Rapier) for debris physics. Latent portals activated on fracture to open hidden areas.

### Rendering and visual polish

- **Billboard sprite rendering** — ~~character and effect sprites; depth-sort against world geometry.~~ **Shipped.** `BillboardEmitter` entity type, particle sim, and additive billboard pass (`src/fx/smoke.rs`, `billboard.wgsl`). See `plans/done/scripting-foundation/plan-3-emitter-entity.md`.
- **Specular maps** — ~~per-texel specular highlights in the direct light loop. Shading model decision (Phong vs. PBR) required first.~~ **Shipped.** Blinn-Phong per-texel specular via `_s.png` siblings, chunk-list multi-source loop, bumped-Lambert correction. See `plans/done/normal-maps/`.
- **Fog volumes** — `env_fog_volume` brush entity fully wired to a runtime fog pass. **Partially implemented** — `src/render/fog_pass.rs` written (634 lines) but not yet imported in `render/mod.rs`. See `plans/in-progress/fx-volumetric-smoke/` Task B.
- **Emissive / fullbright surfaces** — ~~texture prefix or material flag for self-lit surfaces.~~ **Shipped.** `emissive_` prefix → `Material::Emissive`; `emissive_intensity` uniform; bloom-ready bypass in forward shader. See `plans/done/emissive-surfaces/`.
- **Post-processing** — bloom, optional CRT/scanline filter.
- **Baked cubemap reflections** — `env_cubemap` point entity baked to a cubemap atlas at compile time.

### Infrastructure

- **Sector Graph + Portal Culling** — replace BSP-as-runtime-scaffolding with an author-defined sector graph. Latent portals (activate on event) support destruction reveals. Prerequisite for kinematic clusters that need their own sector graphs.
- **Chunk Primitive** — unify static world geometry, kinematic clusters, and dynamic debris into one record type (mesh + collider + transform + sector membership). Deferred until two or more of those consumers exist and the duplication cost is clear.
- **Audio foundation** — kira integration, spatial audio, reverb zones.
- **HUD and UI** — health, ammo, crosshair, menus.
- **Multi-format map support** — UDMF and others via `format/<name>.rs` sibling modules.

### Dropped

- **SDF atlas + sphere-traced soft shadows** — descoped in favor of the lightmap pipeline. Hard shadow edges fit the aesthetic; SDF complexity not justified.
- **Cubemap bake tool** — deferred indefinitely; baked cubemap reflections remain on the speculative list above but the standalone tool is dropped.
