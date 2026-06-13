# M10 — Skeletal Hit Zones

## Goal

Dynamic hittable volumes: bone-parented proxy capsules posed from the skeleton, raycast through a weapon-agnostic entity-raycast facility the weapon is the first consumer of. The model ships the spatial tag (glTF per-node `extras`), the descriptor script ships the balance (per-zone damage multipliers).

## Scope

### In scope

- glTF per-node `extras` reading: a zone tag + capsule radius per joint, carried into the loaded model.
- A game-side CPU model store retaining each loaded model's skeleton, clips, and zone table — the visibility-independent pose source the animation plan deferred to this plan.
- A world-joint pose-sampling path: posed joint world transforms (not skinning matrices), sampled on demand from the entity's live animation state at the shared animation clock.
- A **derived broad-phase bound** per zone-bearing model, computed at load from the model's own clips — never trusted to the authored hitbox.
- A standalone **entity-raycast facility** (AABB entities + posed-capsule entities behind one query) that the weapon hitscan delegates to; AI line-of-sight and projectiles reuse it later without touching weapon code.
- Zone identity on the weapon impact; per-archetype zone multipliers declared in the descriptor (`health` block), applied at the damage-routing site.

### Out of scope

- Zones blocking movement or entity-entity collision — raycast-only, matching the AABB's role.
- Per-frame eager pose caching and distance-based time-slicing — sampling is on-demand at query time; revisit only if profiling shows redundant sampling under sustained fire at wave scale.
- Locational *effects* (gibs, stagger, hit reactions) — only the damage multiplier lands here.
- Projectile / area damage against zones — hitscan is the consumer this plan ships; the query facility is the seam projectiles plug into later.
- Hit zones on the player — the player carries no mesh; the AABB-less status quo stands.
- Authoring tooling / Blender exporter conventions doc (a `docs/` follow-up, not engine work).

## Design decisions

- **Pose source.** Zones are posed by calling the model module's pure samplers directly from the game layer — same clip index, clip-local time, loop policy, and crossfade weight the renderer's sample-params path computes, driven by the same animation clock, so zones never desync from visuals. No second clock, no GPU readback.
- **On-demand sampling.** Poses are computed only when a ray actually queries an entity (weapon fire is at most once per tick per weapon), and only for entities surviving the broad phase. No per-frame harness.
- **Derived broad-phase bound.** For a zone-bearing model the broad-phase AABB is computed once at load: the union of joint positions across every clip sampled at fixed uniform times (deterministic; ≥ 8 samples per clip including both endpoints), inflated by each joint's capsule radius and re-centered on the entity transform. The authored `hitbox` is **not** consulted for zone-bearing entities — an animated limb can never silently swing outside the broad phase. AABB-only entities keep the authored hitbox unchanged.
- **Capsule shape.** A tagged joint's capsule segment runs from the joint's posed origin to its first child joint's posed origin; a tagged leaf joint becomes a sphere (zero-length segment) at the joint origin. Radius comes from the model (`extras`), with an engine default when omitted.
- **Targetability and fallback.** An entity is hitscan-targetable iff it carries health and either an authored hitbox or a zone-bearing model. A model with no tagged joints keeps today's AABB behavior unchanged. For zone-bearing models the capsule test decides the hit; untagged joints are not hittable — authors tag what should register.
- **One query, two narrow phases.** The entity-raycast facility owns the walk over targetable entities, the broad phase, and both narrow phases (AABB slab test; posed-capsule test). It is callable with any origin/direction/range — no weapon types in its signature. The weapon's hitscan delegates to it and keeps only the world-vs-entity nearest-of resolution.
- **Multiplier location.** Zone multipliers live in the descriptor's `health` block (the hitbox precedent) and apply at the existing damage-routing site, scaling the payload amount before the damage chokepoint. `DamagePayload` itself stays amount-only; zone identity rides on the impact, beside the payload — the established invariant.
- **Ray transform.** The capsule test runs against the entity's game-tick transform (position + yaw), consistent with the AABB path; the renderer's interpolated transform is not consulted.

## Acceptance criteria

- [ ] A glTF whose joint nodes carry zone `extras` loads with those zones; malformed or absent per-node `extras` degrades to no zone on that joint, never a load failure.
- [ ] Shooting a zone-bearing entity registers a hit only where the **posed** capsules are: with the clip advanced so a limb has moved, a ray through the limb's rest position misses and a ray through its posed position hits (unit test driving sample time directly).
- [ ] A ray through a posed limb **outside the authored hitbox** still hits — the derived bound covers every clip's pose, so no authored-box sizing can cause a silent miss (unit test with a deliberately undersized authored hitbox).
- [ ] A zone hit reports its zone tag; damage applied equals payload amount × the archetype's multiplier for that tag (e.g. `head: 1.5` drops HP by 1.5× the weapon damage); an unlisted tag applies 1.0.
- [ ] Nearest-of ordering is preserved across world geometry, AABB-only entities, and zone-bearing entities in one scene; a wall in front of a zone still wins; the zero-HP corpse skip still holds.
- [ ] A zone-bearing entity is never hit by a ray that passes inside its broad-phase bound but outside every capsule (the false-positive an AABB alone would report).
- [ ] An entity whose model has no zone tags behaves byte-identically to today: all existing weapon tests pass unmodified.
- [ ] The entity-raycast query is callable directly (no weapon, no camera) with an arbitrary origin/direction/range and returns the same nearest entity hit the weapon path reports for that ray (unit test exercising the facility standalone).
- [ ] A descriptor multiplier naming a tag absent from the entity's model logs one warning at level load and is ignored; the Luau and TS descriptor paths accept the same shape; generated typedefs include the new field.
- [ ] Hot-reloading the descriptor updates multipliers on live entities without respawn.
- [ ] The game-side store, sampling path, and raycast facility import nothing from the renderer module and touch no wgpu types.

## Tasks

### Task 1: Per-node extras → zone table

Extend the glTF loader to read each skeleton joint node's `extras` for `hitZone` (string tag) and `hitZoneRadius` (meters, optional), producing a per-joint zone table parallel to the skeleton's joints, carried on the loaded model beside the existing document-level tags. Per-node keys are a separate namespace from the document-level `tags` key. Same degradation contract as document extras: any malformed value yields no zone for that joint, never an error. Tests cover tagged, untagged, malformed, and leaf-joint cases.

### Task 2: World-joint sampling API

Add a public sampling entry to the model animation module that produces **world-space joint transforms** (one per joint, pre-inverse-bind) for the same inputs the palette samplers take — single clip with loop policy, or a two-source blend at a weight. Reuses the existing forward-sweep compose; the skinning-palette functions are untouched. Allocation contract matches them (caller-reused output buffer, scratch reuse).

### Task 3: Game-side model store + derived bounds

Retain each uploaded model's CPU skeleton, clips, and Task 1's zone table in a game-layer store keyed by model handle, populated at the level-load model sweep beside the existing clip-table install and cleared on level change with it. Share the underlying data with the renderer's copy via `Arc` rather than cloning clip keyframes. For zone-bearing models, compute the derived broad-phase bound here (sample every clip's joint positions at fixed uniform times via Task 2, union, inflate by capsule radii) and store it beside the zone table. This is the visibility-independent pose source; nothing renderer-side is queried per shot.

### Task 4: Entity-raycast facility + weapon integration

A standalone entity-raycast query module: walk targetable entities (health + authored hitbox, or health + zone-bearing model); broad phase via the authored AABB or the derived bound respectively; narrow phase via the existing AABB slab test or the posed-capsule test — resolve the entity's current animation state to sample inputs (the game side's existing state → clip/time/fade resolution), sample world joints via Task 2, build capsules per zone-tagged joint (segment to first child, sphere for leaves, model→world via the entity transform), ray-test each (parry3d). Returns nearest entity hit with point, normal, target, and optional zone tag. The weapon's hitscan moves its entity walk into this facility and delegates, keeping only world-vs-entity nearest-of resolution; the weapon impact gains an optional zone tag, populated only for capsule hits. Plumbing: the facility takes the registry, the Task 3 store, and the animation clock as explicit parameters; the app's weapon-fire tick (which already borrows the registry) passes the store and clock through the weapon tick.

### Task 5: Zone multipliers in the descriptor + damage application

`zoneMultipliers` (tag → factor map, optional) on the health descriptor: Rust struct + validation (factors finite and ≥ 0), both JS and Luau parse arms, typedef generation, hot-reload refresh, and materialization onto the health component. At the app's damage-routing site, scale the payload amount by the target's multiplier for the impact's zone tag (absent zone or absent table → 1.0) before the damage chokepoint. Level-load validation cross-checks declared tags against the spawned model's zone table and warns once per archetype per unknown tag.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — loader and sampler, disjoint files.
**Phase 2 (sequential):** Task 3 — consumes Task 1's zone table type and Task 2's sampler (derived bounds).
**Phase 3 (sequential):** Task 4 — consumes Tasks 2 + 3; touches the weapon module and the app tick.
**Phase 4 (sequential):** Task 5 — consumes Task 4's zone-on-impact; touches the same app damage-routing site Task 4's caller plumbing lands in.

## Rough sketch

- Loader: `model/gltf_loader.rs` (per-node extras beside `read_model_tags`); zone table on `LoadedModel` (e.g. `joint_zones: Vec<Option<JointZone>>`, parallel to `Skeleton::joints`). File is large but test-dominated; extend in place, no split.
- Sampler: `model/anim.rs` — a world-joint sibling of `sample_clip_looped` / `sample_blended` sharing `compose_palette`'s sweep (factor the sweep so one core serves both outputs).
- Store + facility: new `scripting/systems/hit_zones.rs` owning the model store, derived bounds, and the entity-raycast query, installed in `main.rs` at the same sweep that fills `MeshClipTables` (`MeshClipTables::insert` call site); state→sample-input resolution reuses `scripting/systems/mesh_anim.rs` logic against `MeshAnimation` (`scripting/components/mesh.rs`). The weapon's `nearest_entity_hit` / `ray_aabb_slab` logic relocates here (behavior-preserving move) so AABB and capsule paths live behind one query.
- Weapon: `weapon/mod.rs` delegates its entity walk to the facility; `WeaponImpact` gains `zone: Option<String>`; ray-vs-capsule via `parry3d` query against `parry3d::shape::Capsule` (Y-axis convention already used by movement).
- Descriptor: `scripting/data_descriptors.rs` (`HealthDescriptor.zone_multipliers`), `components/health.rs` materialization, `typedef.rs`, hot-reload refresh path; damage application at the `apply_damage` call in the app's weapon-fire tick (`main.rs`).
- Oversized-file flag: `main.rs` is far past the soft threshold; this plan adds only call-site lines there (store install, clock/store args, one multiplier lookup) — no split in this plan, flag carried for a future refactor.
- Engine default capsule radius when `hitZoneRadius` is absent: 0.12 m.
- Derived-bound sampling density: fixed uniform times per clip, ≥ 8 including both endpoints, deterministic — a constraint, not a tuned number; implementer picks the count.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | glTF extras |
|---|---|---|---|---|---|
| zone multipliers | `zone_multipliers` | `"zoneMultipliers"` (descriptor JSON) | `zoneMultipliers` | `zoneMultipliers` | n/a |
| zone tag | `String` (verbatim) | verbatim | verbatim | verbatim | `"hitZone"` value |
| zone radius | `radius: f32` | n/a | n/a | n/a | `"hitZoneRadius"` |

Zone tags are author-defined strings compared verbatim across all boundaries; no casing transform.

## Open questions

- `Arc` granularity in the shared model data (whole model vs. per-part) — implementation detail at code contact; the constraint is no per-shot deep clone of clip keyframes.
- Whether crossfade blending should participate in the derived bound (a blend of two in-bound poses stays in the convex hull of joint positions only per joint, not strictly for capsule sweeps). The bound already unions all clip extremes plus radii; treat as covered unless a counterexample shows up in testing.
