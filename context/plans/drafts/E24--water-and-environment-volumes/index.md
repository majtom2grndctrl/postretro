# E24--water-and-environment-volumes

Brief · resumable · Epic 24 (Environment Volumes & Fluids) · reads: `context/lib/entity_model.md` §4, §5, §7 · `context/lib/movement.md` §2, §4, §7 · `context/lib/networking.md` §What gates, and what replicates instead · `context/lib/build_pipeline.md` §Source-format neutrality, §Compiler pipeline · `context/lib/development_guide.md` §Workspace · `context/lib/rendering_pipeline.md` · `context/lib/audio.md` §6 · read at 2d3fca5f2

## Problem
Owner-requested capability. Mappers want water to swim in and regions with their own gravity, for retro-style set-pieces and quirky experiments. The engine has neither. World gravity is one level-wide scalar, seeded from worldspawn `initialGravity` (stored in the FogVolumes section) and threaded into movement as `gravity: f32`. Collision has no volume query, no contents filtering, no fluid and no swim state. When done:
- A mapper places a fluid volume or a gravity volume. Overlapping volumes combine. A fluid can be any convex shape, floating in mid-air included.
- A player in a fluid swims and can jump from the surface.
- Fluid faces are visible from inside and outside.
- With the eye inside a fluid, the view tints and audio muffles.
- Hitscan crossing a fluid face splashes.
- A player in a gravity volume falls with that volume's gravity.
- A co-op client predicts all of it with no corrections.
The query layer this adds is the first piece of the engine-owned collision substrate described in `research.md` §Substrate direction.

## Decisions
- **New crate `postretro-collision` for the volume substrate.**
  - Pure geometry over level data: no movement, no entities. It sits at physics' layer or below, and physics depends on it.
  - It holds convex leaves (plane set, AABB, contents bitmask) under a BVH built at level load. The leaves answer point-containment, capsule-immersion and ray-crossing queries, each filtered by a contents mask.
  - The tree is generic over its leaf payload, so later triangle leaves can share the implementation without being forced into one shared tree.
  - It is sized for ambitious content, not expected counts: authors multiply volumes, and environment volumes will absorb reverb zones and later clip volumes. Results never depend on traversal order (see the overlap rule), so the structure can be retuned freely.
  - The tree is built at level load from the section's leaves, not baked into PRL. The leaves stay the single source of truth, and node layout stays free to change during active perf work without a format bump or a recompile of every map. A build over thousands of leaves costs well under a millisecond (`research.md` §Bake versus load-time build). The decision is revisited for triangle leaves, where count and tree quality make baking pay.
  - parry is not used. The static trimesh and mover collision stay in `physics::collision` until later E24 steps rewrite them into this crate (`research.md` §Substrate direction).
  - This amends `development_guide.md` §Workspace "Target shape", where physics owns collision. Precedent: `visibility`, a sibling spatial-query crate. Every collision edit still rebuilds physics and everything above it, and the owner accepted that trade for E24.
- **Environment resolution is one pure function, called at movement tick start.**
  - It maps position, capsule, volumes and level default to gravity, fluid and immersion. Host movement, client prediction, reconcile replay and AI agents all call it per tick, per body, at that body's position.
  - Callers pass only the level default where `gravity: f32` runs today.
  - Nothing joins the wire except the `Swim` state.
  - Intents receive the resolved value and never query. Environment is a function of position and static data, not contact along a sweep, so `movement.md` §4's forward-contact rule does not govern it.
- **Purpose-named authoring entities, one canonical environment volume.**
  - The FGD exposes `fluid_volume` (keys `fluid`, `priority`) and `gravity_volume` (keys `gravity`, `priority`). Later regional fields arrive the same way (a `reverb_volume`).
  - The format adapter translates each into the canonical environment-volume record with only its field set. The engine, the PRL section and the overlap rule never see the authoring split (`build_pipeline.md` §Source-format neutrality: the FGD projects canonical archetypes).
  - Each entity's key is required: a blank `fluid` or `gravity` fails the build, naming the entity. Zero gravity is a valid value.
  - A brush that needs both a fluid and gravity is two coincident entities, which per-field resolution combines.
- **Environment volumes are non-solid brush entities.**
  - They are peeled before the BSP, like `trigger_volume`. It never seals cells, blocks portals, or enters static geometry, static collision, lightmaps, SDF or navmesh.
  - Each brush becomes one convex leaf sharing the entity's record.
  - Volumes are static: no movers and no script toggles. A runtime-mutable volume needs the continuous replication lane `plans/done/E15--session-lifecycle` deferred.
- **One regional overlap rule.** For each field (gravity, fluid), the value comes from the highest-priority volume that contains the query point and sets that field. Equal priorities resolve by map entity order, earliest wins. With no such volume, the level default applies.
  - So a pool with no gravity inside a low-gravity room keeps the room's gravity.
  - `audio.md` §6 reverb zones are folded into this rule: reverb becomes a future environment field, replacing the cell-quantized, smallest-zone-wins rule. They are not built yet, so this costs nothing now.
- **Gravity is a vector in the format, straight-down only for now.** The section stores gravity, including the level default, as a vector. The compiler rejects any non-zero gravity whose direction is not straight down, naming the entity; zero is allowed.
  - Player up-reorientation is a later gravity-frame spec. The substrate, contact classification, capsule, camera and navmesh all assume +Y up (`research.md` §Up-axis survey). The validator is the seam that spec removes.
- **Level default gravity moves out of FogVolumes** into the new section's header.
  - The `initialGravity` KVP and the scalar `worldGetGravity` / `worldSetGravity` keep their authoring shape and drive the level default.
  - Volume gravity overrides scripted level gravity inside the volume. A scripted per-area change is `plans/done/M7--gravity-primitives`' anticipated `area.setGravity`; it needs the deferred replication lane and is not built here.
  - A vector script API belongs to the gravity-frame spec.
- **Ownership is split three ways, as a closed field set.**
  - The volume owns the place: shape, priority, gravity, which fluid.
  - A mod-manifest fluid descriptor owns the substance: viscosity, view tint, muffle, surface opacity, enter and exit sounds.
  - The player descriptor's `movement.swim` block owns the swimmer.
  - Amend `entity_model.md` §4, `build_pipeline.md` §Load-time gameplay declarations and `movement.md` §7 at promotion. Maps may additionally author static regional gravity and fluid placement, baked at compile time. Any further regional field needs its own amendment.
  - Maps still cannot mutate gameplay after load or override per-archetype tuning.
  - Gas and haze are not fluids: they stay with fog volumes.
- **Any convex brush can be a fluid.**
  - Immersion is the fraction of the capsule's vertical extent, feet to head, that the fluid hull contains along the capsule's vertical axis.
  - Leaving a floating fluid through any face, bottom included, drops immersion.
  - The fluid surface is every brush face not buried in solid world and not shared with another brush of the same entity, textured from that face.
  - Eye-dependent presentation keys on the camera eye point.
- **Swim is a native movement state.**
  - Entry and exit gate on immersion with descriptor hysteresis (`enterDepth` > `exitDepth`).
  - Swim drag comes from the fluid's viscosity; speeds and acceleration come from the descriptor.
  - Viscosity acts only while swimming. Wading drag is a follow-up.
  - Fluid jump: while swimming with the eye outside the fluid, jump sets upward velocity to `fluidJumpVelocity` and leaves swim. No nearby solid is required.
  - Swim tuning is never map-overridable.
- **Who resolves environment.**
  - Player pawns resolve the full environment.
  - AI agents resolve gravity only: they do not swim, they walk fluid floors, and the navmesh is unchanged.
  - Each particle resolves gravity at its own position on every particle step, so a particle drifting across a boundary changes gravity on that step. Buoyancy scales the resolved vector.
  - The particle sim steps once per rendered frame on frame time, not per fixed tick. `particle_sim::tick` is called from the render-prep path in `main.rs`, and particles are unhashed presentation, so prediction is unaffected.
  - Projectiles apply no gravity today and are untouched.
- **Co-op parity.**
  - The new section, level default gravity included, joins `level_content_digest`. It is level data both peers load, like static collision.
  - Fluid descriptors ride the host tuning payload. The client predicts with host values and never falls back to its own registry (`networking.md` §What gates).
  - The swim state replicates with movement state, which bumps the wire version.
  - Mid-level `worldSetGravity` replication is not owed here. E15 deferred it on mechanism.
- **Presentation.**
  - Fluid faces draw in a renderer-owned translucent pass after opaque forward and before smoke: alpha blend at the fluid's surface opacity, depth test on, depth write off, two-sided, turbulent UV warp.
  - Eye-in-fluid applies the fluid's tint through screen effects, and its muffle through a new primitive-only low-pass on the audio crate, with no kira types crossing the boundary.
  - Player entry and exit play the fluid's sounds.
  - A hitscan ray crossing a fluid face before its blocking hit emits one splash per crossing.
  - All presentation is unhashed.
- **Non-goals.**
  - Fluid damage: E16's open "DoT / environmental death policy" owns it.
  - Refraction and depth absorption: a later renderer plan.
  - Currents.
  - Wading drag.
  - AI swimming.
  - Mover convex collision and static trimesh cleanup: later E24 work.
  - Clip volumes.
  - Volumes on movers.

### Scripting surface
TypeScript shown. The Luau SDK mirrors it with the same names, and both ship.

```ts
// start-script.ts
export default defineMod({
  name: "Dev", id: "dev", version: "0.1",
  fluids: [
    defineFluid({
      name: "water",
      viscosity: 2.0,                      // 1/s drag while swimming; ≥ 0
      viewTint: [0.10, 0.30, 0.40, 0.35],  // rgb + strength in 0..=1
      muffle: 0.8,                         // 0 = none, 1 = maximum low-pass
      surfaceOpacity: 0.6,                 // 0..=1
      enterSound: "sfx/splash_in",         // optional
      exitSound: "sfx/splash_out",         // optional
    }),
  ],
});

// player.ts — inside the player descriptor's `movement`
swim: {
  speed: 4.5,             // m/s target swim speed
  accel: 6.0,             // m/s² toward wish velocity
  sinkSpeed: 0.8,         // m/s drift down with no vertical input
  enterDepth: 0.5,        // immersion fraction that enters swim
  exitDepth: 0.35,        // immersion fraction that leaves swim; < enterDepth
  fluidJumpVelocity: 6.5, // m/s upward on a jump from the surface
},
```

## Acceptance
### Automated
**Queries and resolution**
- [ ] A point strictly inside, exactly on a face, and just outside a convex volume reports inside, inside, outside. A query whose mask excludes fluid never reports a fluid volume.
- [ ] A pool with no gravity, inside a lower-priority room volume with low gravity: a point in both resolves the pool's fluid and the room's gravity. Two equal-priority volumes setting gravity resolve to the earlier one in map order. A point in no volume resolves level defaults.
- [ ] Over a seeded random set of overlapping and nested volumes, every point, capsule and ray query through the BVH returns the same result as a brute-force scan of all leaves. This includes queries on leaf faces and at shared AABB boundaries.
- [ ] Immersion reads 0 with feet at the fluid top, 1 with the head below it, and a proportional value between. In a fluid block floating in mid-air, a capsule poking out the bottom reads the contained fraction only.
**Compiler**
- [ ] Gravity pointing sideways, pointing up, or tilted fails the build and names the entity. Zero and straight-down gravity compile.
- [ ] A fluid volume or a gravity volume spanning a doorway leaves the cell count, portal count, static collision triangles and navmesh identical to the same map without it.
- [ ] A fluid brush face flush against solid world, and a face shared by two brushes of one fluid entity, emit no surface. Every other face of the brush does.
- [ ] A `fluid_volume` naming an undeclared fluid loads as dry and warns, naming the volume and the fluid. A `fluid_volume` with a blank `fluid`, or a `gravity_volume` with a blank `gravity`, fails the build and names the entity.
**Movement**
- [ ] Wading in until immersion passes `enterDepth` enters swim. Rising until it falls below `exitDepth` leaves it. Bobbing between the two thresholds never toggles state. Immersion exactly at `enterDepth` enters.
- [ ] Swimming with the eye above the surface in open water, far from any solid, jump sets upward velocity to `fluidJumpVelocity` and leaves swim. Fully submerged, jump gives no boost.
- [ ] Swimming down out of the bottom of a floating fluid block leaves swim, and level gravity takes over.
- [ ] In a volume with low gravity the player falls at that gravity. With zero gravity, vertical velocity holds. On the first tick after leaving the volume, level gravity applies. An AI agent in the same volume falls at the volume's gravity.
- [ ] A particle inside a low-gravity volume accelerates at that volume's gravity times its buoyancy. On the particle step it drifts out, it takes level gravity. A zero-buoyancy particle floats in every volume.
- [ ] `worldSetGravity` changes gravity outside volumes and leaves a gravity-setting volume's value in force inside it. A map with only `initialGravity` behaves as before. A PRL built before this change fails to load with a recompile message.
**Co-op**
- [ ] In the predict/reconcile harness, a client crossing into a fluid and into a low-gravity volume produces zero reconciliation corrections, including when a correction's replay window spans both crossings.
- [ ] Changing a volume's shape, priority, gravity or fluid name, or the level default gravity, changes the level content digest. Changing a fluid's sounds, tint or opacity does not.
- [ ] A client whose manifest declares different `water` numbers predicts with the host's values.
**Presentation and surface**
- [ ] A hitscan ray from air into a fluid emits exactly one splash, at the crossing. A ray from inside a fluid out through a face emits one. A ray through a floating block emits two. A ray blocked before reaching a fluid face emits none. A ray that never crosses one emits none.
- [ ] The Scripting surface example runs as a `content/dev` fixture in TypeScript and Luau. A fluid with negative viscosity, or a swim block with `exitDepth` ≥ `enterDepth`, is rejected with a warning naming the field.
### Manual
- [ ] Fluid faces read as warped and translucent from inside and outside, a floating block included. They do not z-fight with surrounding geometry. Smoke above a fluid, seen from above, is not dimmed by it, and fog composites over it.
- [ ] The tint and muffle turn on when the eye enters a fluid and off when it leaves, without flicker while treading at the surface.
- [ ] A swim and fluid-jump playtest: the surface jump feels reliable, and ledges within reach are climbable with it.
- [ ] A low-gravity room playtest, including a pool and a floating fluid block inside the room.
- [ ] A co-op client swims and crosses gravity volumes with no visible corrections.
- [ ] Stress run, measure and report.
  - Fixture: `content/dev/maps/stress-env-volumes.map`, generated by `tools/gen_stress_map.py` before this brief (`research.md` §Stress pre-flight). It has 1,000 fluid and gravity volumes, some overlapping and some floating, plus 64 AI agents, 1 player pawn and 10,000 live particles moving through them. There is one player pawn because no in-process multi-pawn route exists. Per-body cost is linear, and the agents dominate the body count.
  - Pinned: the fixture, the owner's Mac, a release build with dev-tools, a fixed `--start-pose`, and `POSTRETRO_CPU_TIMING` windows after warmup. Batch headless runs reject CPU timing.
  - Metrics: environment-resolution CPU time for bodies per tick, as its own `SimStage`; for particles per frame, as a stage nested in `particle_sim`; hitscan fluid-crossing query time; BVH build time at load.
  - These are per-stage numbers, so they stay readable while the fixture's other costs are high. The pre-flight measured AI at about 7.4 ms per tick and particle sim at about 4.5 ms per frame on this map before any volume does anything (`research.md` §Stress pre-flight). Those costs belong to separate optimization work and do not gate this brief. Baseline: `stress-env-volumes-baseline.map`, the same generated map with zero volumes, measured on the same build.
  - Budget (proposed): bodies under 0.25 ms per tick, particle environment resolution under 0.5 ms per frame, BVH build under 2 ms. Results are recorded in the plan of record.

## Path
- Compiler: peel the brush in the `parse.rs` entity sweep, as for `trigger_volume`. `resolve_brush_region_bounds` gives the planes and AABB, and `build_brush_volumes_with_ids` gives textured faces for the surface mesh. `parse.rs` is past 7,000 lines: land the peel in a new `parse/` submodule. Culling buried and shared faces needs a coplanar-overlap test against solid brush faces and sibling fluid faces.
- Movement: call the resolve where `dispatch.rs` receives `gravity` today, and widen that parameter to the resolved environment. `crates/physics/src/movement/mod.rs` is past 6,000 lines. The swim intent is a new `intents/swim.rs`. Reconcile replay (`reconcile.rs`) threads one gravity value today; it passes the level default and resolves per replayed tick.
- `prl_loader.rs` and `sim/mod.rs` are past 800 lines. New loader and tick code land as submodules.
- Surface culling: follow how `KinematicMoverRenderCollector` culls movers by AABB against visible-cell bounds. The smoke pass in `smoke.rs` is the blended-pass template, but water draws before it. The `has_any_sheet` gate does not cover water.
- Chosen shape: a load-time BVH, generic over leaf payload. Rival: a flat list with an AABB prefilter (the fog-volume precedent). It is faster below roughly 50–100 leaves but degrades linearly with author ambition. Another rival is per-cell volume lists, baked like `FogCellMasks`. Rejected: it ties collision to the visibility partition, entities store no cell, and rays cross cells.
- Particles: `particle_sim::tick` takes the scalar `gravity` today. It gains per-particle resolution through the same pure resolve, gravity field only.
- First slice: the crate, section and resolve, proven by a low-gravity room through host movement, prediction and the replay harness. It falsifies the riskiest assumption, that per-tick resolution keeps host and client identical, before any swim or rendering work.

## Open questions
None. The three questions delegated in the first draft were settled by the owner on 2026-10-05 (`research.md` §Settled open questions):
- **Surface lighting:** baked SH ambient plus baked direct SH plus dynamic lights, evaluated per fragment. The pass reuses the smoke pass's camera, lighting and SH bind-group layouts (groups 0, 2, 3) and adds a fluid-local material group. It lands exactly at the 8/8 FRAGMENT storage ceiling that smoke already occupies. Per-fragment evaluation avoids faceting on large fluid faces, which per-vertex lighting (smoke's choice) would show.
- **Fluid-name lookup:** at level install, not compile time. The PRL stores the name as a string; an unknown name warns and loads the volume as dry. The compiler checks only that the key is non-blank.
- **Crate layering:** the `postretro-collision` query core takes its own plain input types (`glam` only). A thin adapter converts loaded level data. `level-compiler` never depends on the crate.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Fluid volume entity | canonical environment-volume record | PRL record (fluid set) | n/a | n/a | `fluid_volume` (brush) |
| Gravity volume entity | canonical environment-volume record | PRL record (gravity set) | n/a | n/a | `gravity_volume` (brush) |
| Priority | `i32` field | PRL `i32` | n/a | n/a | `priority` (default 0) |
| Gravity override | `Option<Vec3>` | PRL presence flag + 3×`f32` | n/a | n/a | `gravity` on `gravity_volume`, required; a vector in map axes, converted by the format adapter |
| Fluid reference | resolved fluid index | PRL string | n/a | n/a | `fluid` on `fluid_volume`, required |
| Fluid registry | manifest field | tuning payload (opaque) | `fluids` / `defineFluid` | `fluids` / `defineFluid` | n/a |
| Fluid fields | descriptor struct | tuning payload | `name`, `viscosity`, `viewTint`, `muffle`, `surfaceOpacity`, `enterSound`, `exitSound` | same | n/a |
| Swim tuning | `PlayerMovementDescriptor` swim block | tuning payload | `movement.swim` (`speed`, `accel`, `sinkSpeed`, `enterDepth`, `exitDepth`, `fluidJumpVelocity`) | same | n/a |
| Swim state | `MovementStateKind::Swim` | movement-state discriminant (wire version bump) | n/a | n/a | n/a |

## Wire format
**EnvironmentVolumes**: a new PRL section, next free id after `CellResidencySet` (51). Little-endian throughout. Mirrors TriggerVolumes (id 44): a leading `u16` version, `u32` counts, `u32` byte-length-prefixed UTF-8 strings.
- Header: `u16` version (1), then `f32×3` level default gravity.
- Then a `u32` volume count. Per volume:
  - `i32` priority;
  - `u8` gravity present (0 or 1), then `f32×3` gravity, always written and zero when absent;
  - a string fluid name, empty = dry;
  - `u32` brush count, then per brush: `f32×3` AABB min, `f32×3` AABB max, `u32` plane count, then per plane `f32×3` normal and `f32` distance (inside when `dot(p, n) − d ≤ 0`);
  - `u32` surface face count, then per face: a `u32` `texture_index` (the level texture table, as KinematicGeometry face meta), a `u32` vertex count, and per vertex `f32×3` position and `f32×2` UV. A dry volume has zero faces.
- An empty list is a zero count. The section is always present, so a level with no volumes still carries its default gravity.

**FogVolumes** drops `initial_gravity` and bumps its version. Old PRLs fail to load with a recompile message, as the missing-gravity path does today.

The movement-state wire discriminant gains `Swim`, and the wire version bumps.
