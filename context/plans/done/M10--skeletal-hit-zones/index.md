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
- Snapshot-fade fidelity in zone poses — the "smooth" crossfade snapshot store is renderer-owned and unreadable game-side; the facility degrades a snapshot fade to its fallback clip (see Task 4). Acceptable: a hit during a sub-200ms crossfade samples the incoming clip, not the captured blend.
- Authoring tooling / Blender exporter conventions doc (a `docs/` follow-up, not engine work).

## Design decisions

- **Pose source.** Zones are posed by calling the model module's pure samplers directly from the game layer — same clip index, clip-local time, loop policy, and crossfade weight the renderer's sample-params path computes, driven by the same animation clock, so zones never desync from visuals. No second clock, no GPU readback. The world-joint sampler **factors** `compose_palette`'s forward sweep into a shared core (today `compose_palette` applies the inverse-bind inline and returns only skinning matrices, discarding the world pose — see Task 2).
- **On-demand sampling.** Poses are computed only when a ray actually queries an entity (weapon fire is at most once per tick per weapon), and only for entities surviving the broad phase. No per-frame harness.
- **Derived broad-phase bound.** For a zone-bearing model the broad-phase AABB is computed once at load: the union of joint positions across every clip sampled at fixed uniform times (deterministic; ≥ 8 samples per clip including both endpoints), inflated by each joint's capsule radius and re-centered on the entity transform. The authored `hitbox` is **not** consulted for zone-bearing entities — an animated limb can never silently swing outside the broad phase. AABB-only entities keep the authored hitbox unchanged.
- **Capsule shape.** A tagged joint's capsule segment runs from the joint's posed origin to its first child joint's posed origin; a tagged leaf joint becomes a sphere (zero-length segment) at the joint origin. Radius comes from the model (`extras`), with an engine default (0.12 m) when omitted. The two posed endpoints define the capsule directly — orientation is arbitrary, so this is not the vertical `+Y` player-sweep convention (`collision/mod.rs`); `parry3d::shape::Capsule::new(a, b, radius)` (segment + radius) and `parry3d::shape::Ball` (leaf) are the shapes, and parry3d is already a workspace dependency (world collision).
- **Targetability and fallback.** An entity is hitscan-targetable iff it carries health and either an authored hitbox or a zone-bearing model. (This widens the `entity_model.md` §7 rule "targetable iff a hitbox is present"; the §7 Health row must be updated in the same implementation pass — primitive surface is a contract.) A model with no tagged joints keeps today's AABB behavior unchanged. For zone-bearing models the capsule test decides the hit; untagged joints are not hittable — authors tag what should register.
- **One query, two narrow phases.** The entity-raycast facility owns the walk over targetable entities, the broad phase, and both narrow phases (AABB slab test; posed-capsule test). It is callable with any origin/direction/range — no weapon types in its signature. The weapon's hitscan delegates to it and keeps only the world-vs-entity nearest-of resolution.
- **Multiplier location.** Zone multipliers live in the descriptor's `health` block (the hitbox precedent) and apply at the existing damage-routing site, scaling the payload amount before the damage chokepoint. `DamagePayload` itself stays amount-only; zone identity rides on the impact, beside the payload — the established invariant.
- **Ray transform.** The capsule test runs against the entity's game-tick `Transform` component (read from the registry), using position and yaw only — no pitch, roll, or scale — consistent with the AABB path; the renderer's `interpolated_transform` is not consulted.

## Acceptance criteria

- [ ] A glTF whose joint nodes carry zone `extras` loads with those zones; malformed or absent per-node `extras` degrades to no zone on that joint, never a load failure (runnable, mirroring the existing malformed-extras loader tests).
- [ ] The world-joint sampler's output, multiplied per joint by that joint's inverse-bind matrix, equals the palette sampler's output for the same inputs (single clip with loop policy, and a two-source blend at a weight) — tested with a **non-identity** inverse-bind so the comparison is meaningful — with the same caller-reused-buffer allocation contract (runnable unit test).
- [ ] Shooting a zone-bearing entity registers a hit only where the **posed** capsules are: with the clip advanced so a limb has moved, a ray through the limb's rest position misses and a ray through its posed position hits (runnable unit test driving sample time directly).
- [ ] A ray through a posed limb **outside an authored-hitbox-sized box** still hits — the derived bound (computed from the clips, not the authored hitbox) is what admits the ray, so no authored-box sizing can cause a silent miss (runnable unit test with a deliberately undersized reference box).
- [ ] A zone hit reports its zone tag; damage applied equals payload amount × the archetype's multiplier for that tag (e.g. `head: 1.5` drops HP by 1.5× the weapon damage); an unlisted tag applies 1.0 (runnable).
- [ ] Nearest-of ordering is preserved across world geometry, AABB-only entities, and zone-bearing entities in one scene; a wall in front of a zone still wins; the zero-HP corpse skip still holds (runnable).
- [ ] A zone-bearing entity is never hit by a ray that passes inside its broad-phase bound but outside every capsule (runnable — the key two-phase correctness test).
- [ ] An entity whose model has no zone tags behaves byte-identically to today: all existing weapon tests pass unmodified (`WeaponImpact` is never literal-constructed in tests — only returned from `tick` — so adding a `zone` field touches only the production construction site; see the `Copy`-removal note in Task 4).
- [ ] The entity-raycast query is callable directly (no weapon, no camera) with an arbitrary origin/direction/range and returns the same nearest entity hit the weapon path reports for that ray (runnable unit test exercising the facility standalone).
- [ ] A `zoneMultipliers` value that is structurally invalid (a factor non-finite or < 0) is a descriptor validation error at parse (runnable, mirroring the existing non-positive-max rejection test). A well-formed entry naming a tag absent from the entity's model is collected and warned once per archetype — the cross-check **returns** the unknown tags (mirroring `resolve_state_clips` → `MissingClip`) so the set is unit-testable; the warn is a thin caller. The Luau and TS descriptor paths accept the same shape; generated typedefs include the new field.
- [ ] Hot-reloading the descriptor updates multipliers on live entities without respawn (runnable at the component level: `HealthComponent::refresh_from_descriptor` reseeds the multiplier map, mirroring the existing refresh tests).
- [ ] The game-side store, sampling path, and raycast facility import nothing from the renderer module and touch no wgpu types (review-gate: grep `crate::render` / `wgpu` in the new modules; the model module is wgpu-free by contract).

## Tasks

### Task 1: Per-node extras → zone table

Extend the glTF loader to read each skeleton joint node's `extras` (via `gltf::Node::extras()`, **not** the document-level `document.as_json().extras` that `read_model_tags` / `ModelExtras` read) for `hitZone` (string tag) and `hitZoneRadius` (meters, `Option<f32>`, stored as-is with no default applied — the 0.12 m default is applied by the consumer in Tasks 3/4). Produce a per-joint zone table and add it as a new field on `LoadedModel`, parallel to `Skeleton::joints`. **The read must happen inside `build_skeleton` and be reindexed through the same skin-joint → topo remap (`skin_joint_to_topo`) that `rest_locals` use**, so the table aligns with the topologically-reordered final `Skeleton.joints` — a naive read in `skin.joints()` order would be mis-indexed. Same degradation contract as document extras: any malformed value yields no zone for that joint, never an error. Tests cover tagged, untagged, malformed, and leaf-joint cases.

### Task 2: World-joint sampling API

Factor `compose_palette`'s parent-before-child forward sweep into a shared core, then add a public sampling entry to the model animation module that produces **world-space joint transforms** — one `glam::Mat4` per joint, pre-inverse-bind — for the same inputs the palette samplers take (single clip with loop policy, or a two-source blend at a weight). `compose_palette` today applies the inverse-bind inline and returns only skinning matrices, so the world pose must be exposed by the factored core, not recovered after the fact; the skinning-palette functions (`sample_clip_looped`, `sample_blended`) keep their current behavior through the shared core. Reuse the existing `WORLD_POSE_SCRATCH` thread-local; allocation contract matches the palette samplers (caller-reused output buffer, scratch reuse). Operates on `Skeleton` (in `model/skeleton.rs`); does not touch `LoadedModel`, so it is disjoint from Task 1.

### Task 3: Game-side model store + derived bounds

Retain each model's CPU skeleton, clips, and Task 1's zone-table field (on `LoadedModel`) in a game-layer store keyed by `ModelHandle`, populated at the level-load model sweep (the `MeshClipTables::insert` call site) and cleared on level change with it. **Source of the CPU model:** today the renderer's `load_skinned_model` destructures the `LoadedModel`, moves `skeleton` + `clips` into the renderer by value, and returns only `tags` — so the sweep site has no CPU model to store. Per the locked independent-ownership decision, the game side obtains its own copy: re-load the model game-side via `gltf_loader::load_model` against the resolved open path (`resolve_model_open_path_and_handle`), keeping the full `LoadedModel` (skeleton + clips + zone table). The store owns this data independently, `Arc`-wrapping clip/skeleton data internally so the per-shot path never deep-clones keyframes. The store is keyed by `ModelHandle`, so it holds one copy per model *type* (not per instance); waves are many instances of few archetypes, so the duplication against the renderer's copy is a handful of models — trivially small.

For zone-bearing models, compute the derived broad-phase bound here using the world-joint sampler added to `model::anim` (Task 2): sample every clip's joint positions at **fixed uniform times — ≥ 8 per clip including both endpoints, deterministic** — union the joint positions, inflate by each joint's capsule radius (the `extras` radius, or the **engine default 0.12 m** when omitted), and store the AABB beside the zone table. This is the visibility-independent pose source; nothing renderer-side is queried per shot.

### Task 4: Entity-raycast facility + weapon integration

A standalone entity-raycast query module (new `scripting/systems/hit_zones.rs`): walk targetable entities (health + authored hitbox, or health + zone-bearing model); broad phase via the authored AABB or the derived bound respectively; narrow phase via the existing AABB slab test or the posed-capsule test.

To sample a zone-bearing entity, resolve its current `MeshAnimation` (`scripting/components/mesh.rs`) to a render-free `(clip_index, clip-local time, loop_policy, blend weight)` plus, for an active fade, the **from-leg as a clip** — degrading a `FadeSource::Snapshot` to its carried `fallback` clip, because the snapshot store is renderer-owned and unreadable game-side (a snapshot fade samples the incoming clip; see the Out-of-scope note). Sample world joints via Task 2, build capsules per zone-tagged joint (segment to first child, sphere for leaves, model→world via the entity's game-tick `Transform` using position + yaw only — not the renderer's `interpolated_transform`), with the engine-default 0.12 m radius for any joint whose `extras` omitted `hitZoneRadius`. Ray-test each with `parry3d` ray vs. `Capsule::new(a, b, radius)` / `Ball` (segment-defined, arbitrary orientation — not the `+Y` `cast_capsule` path).

**Resolution plumbing:** `scripting/systems/mesh_anim.rs::animate_entity` already computes that tuple per state but returns it as render-namespaced types (`ClipSample` / `MeshSampleParams` / `FadeSource` from `render::mesh_instances`). Relocate those sample-param type definitions into a render-free module both the renderer's sample-params builder and this facility import (the renderer updates its `use` sites in the same pass — `mesh_pass.rs`), so the facility imports no renderer types and honors the no-renderer-import AC. (They are plain data, not GPU types, so this respects the renderer-owns-GPU boundary.)

Returns nearest entity hit with point, normal, target, and optional zone tag. The weapon's hitscan moves its `nearest_entity_hit` / `ray_aabb_slab` logic into this facility (behavior-preserving relocation) and delegates, keeping only world-vs-entity nearest-of resolution. **`WeaponImpact` gains `zone: Option<String>`** — note this removes `derive(Copy)` from `WeaponImpact` and from `WeaponFireEvents` (which embeds `Option<WeaponImpact>`); audit the `Copy`-dependent uses (e.g. `run_weapon_fire_tick` at `main.rs:2701-2713` reuses `events` after moving `impact` out) and adjust to moves/borrows. The sole literal construction site is production (`fire_hitscan` in `weapon/mod.rs`); tests only read `events.impact`, so no test edits. Plumbing: the facility takes the registry, the Task 3 store, and the animation clock as explicit parameters; add the Task 3 store as an `App` field alongside `self.collision_world` and thread it plus the clock through the weapon `tick` from `run_weapon_fire_tick`.

### Task 5: Zone multipliers in the descriptor + damage application

Add `zone_multipliers` (`#[serde(default, rename = "zoneMultipliers")]`, tag → factor map) to `HealthDescriptor` in `scripting/data_descriptors.rs`. `HealthDescriptor` deserializes via `serde_json::from_value` in **both** the JS and Luau parse arms, so the field is accepted in both for free — the only hand-written work is the validation and the typedefs. Add the validation to `HealthDescriptor::validate` (each factor finite and ≥ 0 → a parse-time `DescriptorError`). The typedefs in `scripting/typedef.rs` are **hand-maintained literal string blocks**, not generated — edit the `HealthDescriptor` block in both the TS (`.d.ts`) and Luau (`.d.luau`) builder arms. Materialize onto `HealthComponent` (`scripting/components/health.rs`): add a `zone_multipliers` field, set it in both `from_descriptor` and `refresh_from_descriptor` (the latter is the hot-reload reseed; the existing archetype hot-reload sweep that calls `refresh_from_descriptor` — grep its callers — then carries the new map onto live entities without respawn).

At the app's damage-routing site (the `apply_damage` call in `run_weapon_fire_tick`, `main.rs`), read `impact.zone` (added in Task 4) and the target's `HealthComponent.zone_multipliers`, scale a fresh `DamagePayload.amount` by the multiplier for that tag (absent zone or absent entry → 1.0) before the chokepoint. Level-load validation cross-checks well-formed declared tags against the spawned model's zone table (via the Task 3 store) and **returns** the unknown tags so the caller warns once per archetype per unknown tag — model it on `resolve_mesh_entity_clips` (the level-load, warn-once-per-missing-clip precedent in `main.rs`, which runs after the model sweep with the store available).

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — loader (`gltf_loader.rs` / `LoadedModel`) and sampler (`anim.rs` / `Skeleton`), disjoint files and types.
**Phase 2 (sequential):** Task 3 — consumes Task 1's `LoadedModel` zone-table field and Task 2's `model::anim` world-joint sampler (derived bounds).
**Phase 3 (sequential):** Task 4 — consumes Tasks 2 + 3; touches the weapon module, relocates the `mesh_anim.rs` sample-param types (renderer import update), and the app tick.
**Phase 4 (sequential):** Task 5 — consumes Task 4's zone-on-impact; touches the same `run_weapon_fire_tick` damage-routing site Task 4's caller plumbing lands in.

## Rough sketch

- Loader: `model/gltf_loader.rs` (per-node `gltf::Node::extras()` inside `build_skeleton`, topo-reindexed); zone table on `LoadedModel` (e.g. `joint_zones: Vec<Option<JointZone>>`, parallel to `Skeleton::joints`). File is large but test-dominated; extend in place, no split.
- Sampler: `model/anim.rs` — a world-joint sibling of `sample_clip_looped` / `sample_blended` over a factored-out forward-sweep core, emitting `Vec<glam::Mat4>` world poses.
- Store + facility: new `scripting/systems/hit_zones.rs` owning the model store (re-loaded `LoadedModel`s), derived bounds, and the entity-raycast query, installed in `main.rs` at the `MeshClipTables` sweep; render-free state→tuple resolution factored from `scripting/systems/mesh_anim.rs` (its sample-param types relocate out of `render::mesh_instances`). The weapon's `nearest_entity_hit` / `ray_aabb_slab` logic relocates here.
- Weapon: `weapon/mod.rs` delegates its entity walk to the facility; `WeaponImpact` gains `zone: Option<String>` (drops `Copy`); ray-vs-capsule via `parry3d` `Capsule`/`Ball`.
- Descriptor: `scripting/data_descriptors.rs` (`HealthDescriptor.zone_multipliers` + `validate`), `components/health.rs` materialization + `refresh_from_descriptor`, `typedef.rs` (hand-edited TS + Luau blocks), the archetype hot-reload sweep; damage application at the `apply_damage` call in `run_weapon_fire_tick` (`main.rs`).
- Oversized-file flag: `main.rs` is far past the soft threshold; this plan adds only call-site lines there (store install, clock/store args, one multiplier lookup) — no split in this plan, flag carried for a future refactor.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | glTF extras |
|---|---|---|---|---|---|
| zone multipliers | `zone_multipliers` | `"zoneMultipliers"` (descriptor JSON) | `zoneMultipliers` | `zoneMultipliers` | n/a |
| zone tag | `String` (verbatim) | n/a (keys of `zoneMultipliers`) | n/a (keys of `zoneMultipliers`) | n/a (keys of `zoneMultipliers`) | `"hitZone"` value |
| zone radius | `radius: f32` | n/a | n/a | n/a | `"hitZoneRadius"` |

Zone tags are author-defined strings compared verbatim across all boundaries; no casing transform. The tag has no standalone scripting field — it appears only as a glTF `extras` value and as `zoneMultipliers` map keys.

## Open questions

(None — see Resolved during review.)

## Resolved during review

- Crossfade and the derived bound: the bound unions per-clip pose extremes plus capsule radii; crossfade transients (a blend of two in-bound poses) are treated as covered. Not a separate sampling pass.
- Snapshot fades: the facility degrades a `FadeSource::Snapshot` to its `fallback` clip because the snapshot store is renderer-owned and unreadable game-side — a hit during a sub-200ms crossfade samples the incoming clip (acceptable; see Out of scope).
- Ownership / `Arc` sharing: the game store owns its CPU model data independently (whole-model `Arc`, re-loaded via `gltf_loader::load_model`, no sharing with the renderer's copy). Stakes are O(model types), not O(instances), so the duplication is negligible at wave scale; the subsystem-boundary invariant and the lean north star favor independence over a trivial memory saving. Decided now rather than deferred because nothing downstream resolves it — unlike nav's fragmentation question, it has no consumer that produces deciding data.

## Implementation deviations

- **Task 5 typedef mechanism (deviation from spec).** The Task 5 description and Rough sketch stated that `typedef.rs` contains hand-maintained literal string blocks for TS and Luau types. This was incorrect. The SDK typedefs are registry-generated: field registration in `scripting/primitives/mod.rs` drives `rust_to_ts` / `rust_to_luau` mappings, and golden snapshot tests (`committed_sdk_types_match_current_registry`) enforce that `sdk/types/*.d.ts` and `sdk/types/*.d.luau` match the registry output. The `zoneMultipliers` field was added through the generator; the committed SDK type files were regenerated. No hand-edited literal blocks exist or were modified.
