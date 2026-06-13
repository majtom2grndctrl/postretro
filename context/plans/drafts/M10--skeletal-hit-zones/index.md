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

- **Pose source.** Zones are posed by calling the model module's pure samplers directly from the game layer — same clip index, clip-local time, loop policy, and crossfade weight the renderer's sample-params path computes, driven by the same animation clock, so zones never desync from visuals. No second clock, no GPU readback. The world-joint sampler **factors** `compose_palette`'s forward sweep into a shared core (today `compose_palette` applies the inverse-bind inline and returns only skinning matrices, discarding the world pose — see Task 2).
- **On-demand sampling.** Poses are computed only when a ray actually queries an entity (weapon fire is at most once per tick per weapon), and only for entities surviving the broad phase. No per-frame harness.
- **Derived broad-phase bound.** For a zone-bearing model the broad-phase AABB is computed once at load: the union of joint positions across every clip sampled at fixed uniform times (deterministic; ≥ 8 samples per clip including both endpoints), inflated by each joint's capsule radius and re-centered on the entity transform. The authored `hitbox` is **not** consulted for zone-bearing entities — an animated limb can never silently swing outside the broad phase. AABB-only entities keep the authored hitbox unchanged.
- **Capsule shape.** A tagged joint's capsule segment runs from the joint's posed origin to its first child joint's posed origin; a tagged leaf joint becomes a sphere (zero-length segment) at the joint origin. Radius comes from the model (`extras`), with an engine default when omitted. The two posed endpoints define the capsule directly — orientation is arbitrary, so this is not the vertical `+Y` player-sweep convention (`collision/mod.rs`); `parry3d::shape::Capsule` (segment + radius) and `parry3d::shape::Ball` (leaf) are the shapes, and parry3d is already a workspace dependency (world collision).
- **Targetability and fallback.** An entity is hitscan-targetable iff it carries health and either an authored hitbox or a zone-bearing model. (This widens the `entity_model.md` §7 rule "targetable iff a hitbox is present"; the §7 Health row must be updated in the same implementation pass — primitive surface is a contract.) A model with no tagged joints keeps today's AABB behavior unchanged. For zone-bearing models the capsule test decides the hit; untagged joints are not hittable — authors tag what should register.
- **One query, two narrow phases.** The entity-raycast facility owns the walk over targetable entities, the broad phase, and both narrow phases (AABB slab test; posed-capsule test). It is callable with any origin/direction/range — no weapon types in its signature. The weapon's hitscan delegates to it and keeps only the world-vs-entity nearest-of resolution.
- **Multiplier location.** Zone multipliers live in the descriptor's `health` block (the hitbox precedent) and apply at the existing damage-routing site, scaling the payload amount before the damage chokepoint. `DamagePayload` itself stays amount-only; zone identity rides on the impact, beside the payload — the established invariant.
- **Ray transform.** The capsule test runs against the entity's game-tick transform, using position and yaw only — no pitch, roll, or scale — consistent with the AABB path; the renderer's interpolated transform is not consulted.

## Acceptance criteria

- [ ] A glTF whose joint nodes carry zone `extras` loads with those zones; malformed or absent per-node `extras` degrades to no zone on that joint, never a load failure.
- [ ] The world-joint sampler returns pre-inverse-bind world transforms that match the palette path's pose for the same inputs (single clip with loop policy, and a two-source blend at a weight), with the same caller-reused-buffer allocation contract (unit test against the palette output).
- [ ] Shooting a zone-bearing entity registers a hit only where the **posed** capsules are: with the clip advanced so a limb has moved, a ray through the limb's rest position misses and a ray through its posed position hits (unit test driving sample time directly).
- [ ] A ray through a posed limb **outside the authored hitbox** still hits — the derived bound covers every clip's pose, so no authored-box sizing can cause a silent miss (unit test with a deliberately undersized authored hitbox).
- [ ] A zone hit reports its zone tag; damage applied equals payload amount × the archetype's multiplier for that tag (e.g. `head: 1.5` drops HP by 1.5× the weapon damage); an unlisted tag applies 1.0.
- [ ] Nearest-of ordering is preserved across world geometry, AABB-only entities, and zone-bearing entities in one scene; a wall in front of a zone still wins; the zero-HP corpse skip still holds.
- [ ] A zone-bearing entity is never hit by a ray that passes inside its broad-phase bound but outside every capsule (the false-positive an AABB alone would report).
- [ ] An entity whose model has no zone tags behaves byte-identically to today: all existing weapon tests pass unmodified (`WeaponImpact` is never literal-constructed in tests — only returned from `tick` — so adding a `zone` field touches only the production construction site).
- [ ] The entity-raycast query is callable directly (no weapon, no camera) with an arbitrary origin/direction/range and returns the same nearest entity hit the weapon path reports for that ray (unit test exercising the facility standalone).
- [ ] A `zoneMultipliers` value that is structurally invalid (a factor non-finite or < 0) is a descriptor validation error at parse; a well-formed entry naming a tag absent from the entity's model logs one warning at level load and is ignored. The Luau and TS descriptor paths accept the same shape; generated typedefs include the new field.
- [ ] Hot-reloading the descriptor updates multipliers on live entities without respawn.
- [ ] The game-side store, sampling path, and raycast facility import nothing from the renderer module and touch no wgpu types.

## Tasks

### Task 1: Per-node extras → zone table

Extend the glTF loader to read each skeleton joint node's `extras` for `hitZone` (string tag) and `hitZoneRadius` (meters, optional), producing a per-joint zone table parallel to the skeleton's joints, added as a new field on the loaded-model struct (`LoadedModel`) beside the existing document-level `tags` — not transient loader state. Per-node keys are a separate namespace from the document-level `tags` key (read today by `read_model_tags` / `ModelExtras`). Same degradation contract as document extras: any malformed value yields no zone for that joint, never an error. Tests cover tagged, untagged, malformed, and leaf-joint cases.

### Task 2: World-joint sampling API

Factor `compose_palette`'s parent-before-child forward sweep into a shared core, then add a public sampling entry to the model animation module that produces **world-space joint transforms** (one per joint, pre-inverse-bind) for the same inputs the palette samplers take — single clip with loop policy, or a two-source blend at a weight. `compose_palette` today applies the inverse-bind inline and returns only skinning matrices, so the world pose must be exposed by the factored core, not recovered after the fact; the skinning-palette functions keep their current behavior through the shared core. Allocation contract matches the palette samplers (caller-reused output buffer, scratch reuse). Operates on `Skeleton` (in `model/skeleton.rs`); does not touch `LoadedModel`, so it is disjoint from Task 1.

### Task 3: Game-side model store + derived bounds

Retain each uploaded model's CPU skeleton, clips, and Task 1's zone table in a game-layer store keyed by `ModelHandle`, populated at the level-load model sweep beside the existing clip-table install (`MeshClipTables::insert` call site) and cleared on level change with it. The store **owns its CPU model data independently** — no sharing with the renderer's CPU clip copy — `Arc`-wrapping clip/skeleton data internally so the per-shot path never deep-clones keyframes. The store is keyed by `ModelHandle`, so it holds one copy per model *type* (not per instance); waves are many instances of few archetypes, so the duplication against the renderer's copy is a handful of models — trivially small, and worth it to keep the subsystem boundary clean (the game store never reaches into renderer ownership). For zone-bearing models, compute the derived broad-phase bound here (sample every clip's joint positions at fixed uniform times via Task 2, union, inflate by capsule radii) and store it beside the zone table. This is the visibility-independent pose source; nothing renderer-side is queried per shot.

### Task 4: Entity-raycast facility + weapon integration

A standalone entity-raycast query module: walk targetable entities (health + authored hitbox, or health + zone-bearing model); broad phase via the authored AABB or the derived bound respectively; narrow phase via the existing AABB slab test or the posed-capsule test. To sample a zone-bearing entity, resolve its current `MeshAnimation` to a render-free `(clip_index, clip-local time, loop_policy, blend weight)` tuple, sample world joints via Task 2, build capsules per zone-tagged joint (segment to first child, sphere for leaves, model→world via the entity transform using position + yaw only), and ray-test each (`parry3d` ray vs. `Capsule`/`Ball`). **Resolution plumbing:** `mesh_anim.rs` already computes that tuple per state but packages it into render-namespaced types (`ClipSample` / `MeshSampleParams` from `render::mesh_instances`); factor the pure resolution into a render-free helper both the renderer's sample-params builder and this facility call, so the facility imports no renderer types (honoring the no-renderer-import AC). Returns nearest entity hit with point, normal, target, and optional zone tag. The weapon's hitscan moves its `nearest_entity_hit` / `ray_aabb_slab` logic into this facility (behavior-preserving relocation) and delegates, keeping only world-vs-entity nearest-of resolution; `WeaponImpact` gains `zone: Option<String>` (the sole literal construction site is production in `weapon/mod.rs`; tests are unaffected). Plumbing: the facility takes the registry, the Task 3 store, and the animation clock as explicit parameters; the app's weapon-fire tick (which already borrows the registry) passes the store and clock through the weapon tick.

### Task 5: Zone multipliers in the descriptor + damage application

`zoneMultipliers` (tag → factor map, optional) on the health descriptor: Rust struct + validation (factors finite and ≥ 0 — a violation is a parse-time descriptor error), both JS and Luau parse arms, typedef generation, hot-reload refresh, and materialization onto the health component. The hot-reload path re-applies `zoneMultipliers` to already-spawned health components of the archetype (the descriptor hot-reload sweep overwrites the materialized multiplier map, mirroring the existing `HealthComponent::refresh_from_descriptor` reseed), so live entities update without respawn. At the app's damage-routing site (the `apply_damage` call in the weapon-fire tick), scale the payload amount by the target's multiplier for the impact's zone tag (absent zone or absent table → 1.0) before the damage chokepoint. Level-load validation cross-checks well-formed declared tags against the spawned model's zone table and warns once per archetype per unknown tag.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — loader (`gltf_loader.rs`/`LoadedModel`) and sampler (`anim.rs`/`Skeleton`), disjoint files and types.
**Phase 2 (sequential):** Task 3 — consumes Task 1's zone table type and Task 2's sampler (derived bounds).
**Phase 3 (sequential):** Task 4 — consumes Tasks 2 + 3; touches the weapon module, `mesh_anim.rs` (resolution factoring), and the app tick.
**Phase 4 (sequential):** Task 5 — consumes Task 4's zone-on-impact; touches the same app damage-routing site Task 4's caller plumbing lands in.

## Rough sketch

- Loader: `model/gltf_loader.rs` (per-node extras beside `read_model_tags`); zone table on `LoadedModel` (e.g. `joint_zones: Vec<Option<JointZone>>`, parallel to `Skeleton::joints`). File is large but test-dominated; extend in place, no split.
- Sampler: `model/anim.rs` — a world-joint sibling of `sample_clip_looped` / `sample_blended` over a factored-out forward-sweep core (`compose_palette` applies inverse-bind inline today, so the sweep must be lifted into a shared core that can emit either world poses or skinning matrices).
- Store + facility: new `scripting/systems/hit_zones.rs` owning the model store, derived bounds, and the entity-raycast query, installed in `main.rs` at the same sweep that fills `MeshClipTables`; render-free state→tuple resolution factored from `scripting/systems/mesh_anim.rs` (which currently emits `render::mesh_instances` types) against `MeshAnimation` (`scripting/components/mesh.rs`). The weapon's `nearest_entity_hit` / `ray_aabb_slab` logic relocates here (behavior-preserving move) so AABB and capsule paths live behind one query.
- Weapon: `weapon/mod.rs` delegates its entity walk to the facility; `WeaponImpact` gains `zone: Option<String>`; ray-vs-capsule via `parry3d` against `Capsule` (two posed endpoints, arbitrary orientation) and `Ball` (leaf joints).
- Descriptor: `scripting/data_descriptors.rs` (`HealthDescriptor.zone_multipliers`, both serde-`camelCase` parse arms — consistent with the existing `halfExtents`/`fireRateMs` casing), `components/health.rs` materialization + `refresh_from_descriptor`, `typedef.rs` (TS + Luau), hot-reload refresh path; damage application at the `apply_damage` call in the app's weapon-fire tick (`main.rs`).
- Oversized-file flag: `main.rs` is far past the soft threshold; this plan adds only call-site lines there (store install, clock/store args, one multiplier lookup) — no split in this plan, flag carried for a future refactor.
- Engine default capsule radius when `hitZoneRadius` is absent: 0.12 m.
- Derived-bound sampling density: fixed uniform times per clip, ≥ 8 including both endpoints, deterministic — a constraint, not a tuned number; implementer picks the count.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | glTF extras |
|---|---|---|---|---|---|
| zone multipliers | `zone_multipliers` | `"zoneMultipliers"` (descriptor JSON) | `zoneMultipliers` | `zoneMultipliers` | n/a |
| zone tag | `String` (verbatim) | n/a (keys of `zoneMultipliers`) | n/a (keys of `zoneMultipliers`) | n/a (keys of `zoneMultipliers`) | `"hitZone"` value |
| zone radius | `radius: f32` | n/a | n/a | n/a | `"hitZoneRadius"` |

Zone tags are author-defined strings compared verbatim across all boundaries; no casing transform. The tag has no standalone scripting field — it appears only as a glTF `extras` value and as `zoneMultipliers` map keys.

## Resolved during review

- Crossfade and the derived bound: the bound unions per-clip pose extremes plus capsule radii; crossfade transients (a blend of two in-bound poses) are treated as covered. Not a separate sampling pass.
- Ownership / `Arc` sharing: the game store owns its CPU model data independently (whole-model `Arc`, no sharing with the renderer's copy). Stakes are O(model types), not O(instances), so the duplication is negligible at wave scale; the subsystem-boundary invariant and the lean north star favor independence over a trivial memory saving. Decided now rather than deferred because nothing downstream resolves it — unlike nav's fragmentation question, it has no consumer that produces deciding data.
