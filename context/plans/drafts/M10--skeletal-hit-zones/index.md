# M10 — Skeletal Hit Zones

## Goal

Dynamic hittable volumes: bone-parented proxy capsules posed from the skeleton, raycast by the weapon alongside (and, when present, instead of) the static per-archetype AABB. The model ships the spatial tag (glTF per-node `extras`), the descriptor script ships the balance (per-zone damage multipliers).

## Scope

### In scope

- glTF per-node `extras` reading: a zone tag + capsule radius per joint, carried into the loaded model.
- A game-side CPU model store retaining each loaded model's skeleton, clips, and zone table — the visibility-independent pose source the animation plan deferred to this plan.
- A world-joint pose-sampling path: posed joint world transforms (not skinning matrices), sampled on demand from the entity's live animation state at the shared animation clock.
- Posed-capsule raycast in the weapon hitscan: zones replace the AABB as the precise test when a model carries them; the AABB remains both the no-zones fallback and the broad-phase reject.
- Zone identity on the weapon impact; per-archetype zone multipliers declared in the descriptor (`health` block), applied at the damage-routing site.

### Out of scope

- Zones blocking movement or entity-entity collision — raycast-only, matching the AABB's role.
- Per-frame eager pose caching and distance-based time-slicing — sampling is on-demand at query time; revisit only if profiling shows redundant sampling under sustained fire at wave scale.
- Locational *effects* (gibs, stagger, hit reactions) — only the damage multiplier lands here.
- Projectile / area damage against zones — hitscan only, like the weapon.
- Hit zones on the player — the player carries no mesh; the AABB-less status quo stands.
- Authoring tooling / Blender exporter conventions doc (a `docs/` follow-up, not engine work).

## Design decisions

- **Pose source.** Zones are posed by calling the model module's pure samplers directly from the game layer — same clip index, clip-local time, loop policy, and crossfade weight the renderer's sample-params path computes, driven by the same animation clock, so zones never desync from visuals. No second clock, no GPU readback.
- **On-demand sampling.** Poses are computed only when a ray actually queries an entity (weapon fire is at most once per tick per weapon), and only for entities surviving the AABB broad phase. No per-frame harness.
- **Capsule shape.** A tagged joint's capsule segment runs from the joint's posed origin to its first child joint's posed origin; a tagged leaf joint becomes a sphere (zero-length segment) at the joint origin. Radius comes from the model (`extras`), with an engine default when omitted.
- **Fallback semantics.** A model with no tagged joints keeps today's AABB behavior unchanged. When tags exist, the AABB becomes broad-phase only; the capsule hit decides. Untagged joints are not hittable — authors tag what should register.
- **Multiplier location.** Zone multipliers live in the descriptor's `health` block (the hitbox precedent) and apply at the existing damage-routing site, scaling the payload amount before the damage chokepoint. `DamagePayload` itself stays amount-only; zone identity rides on the impact, beside the payload — the established invariant.
- **Ray transform.** The capsule test runs against the entity's game-tick transform (position + yaw), consistent with the AABB path; the renderer's interpolated transform is not consulted.

## Acceptance criteria

- [ ] A glTF whose joint nodes carry zone `extras` loads with those zones; malformed or absent per-node `extras` degrades to no zone on that joint, never a load failure.
- [ ] Shooting a zone-bearing entity registers a hit only where the **posed** capsules are: with the clip advanced so a limb has moved, a ray through the limb's rest position misses and a ray through its posed position hits (unit test driving sample time directly).
- [ ] A zone hit reports its zone tag; damage applied equals payload amount × the archetype's multiplier for that tag (e.g. `head: 1.5` drops HP by 1.5× the weapon damage); an unlisted tag applies 1.0.
- [ ] Nearest-of ordering is preserved across world geometry, AABB-only entities, and zone-bearing entities in one scene; a wall in front of a zone still wins; the zero-HP corpse skip still holds.
- [ ] A zone-bearing entity is never hit by a ray that passes inside its broad-phase AABB but outside every capsule (the false-positive the AABB alone would report).
- [ ] An entity whose model has no zone tags behaves byte-identically to today: all existing weapon tests pass unmodified.
- [ ] A descriptor multiplier naming a tag absent from the entity's model logs one warning at level load and is ignored; the Luau and TS descriptor paths accept the same shape; generated typedefs include the new field.
- [ ] Hot-reloading the descriptor updates multipliers on live entities without respawn.
- [ ] The game-side store and sampling path import nothing from the renderer module and touch no wgpu types.

## Tasks

### Task 1: Per-node extras → zone table

Extend the glTF loader to read each skeleton joint node's `extras` for `hitZone` (string tag) and `hitZoneRadius` (meters, optional), producing a per-joint zone table parallel to the skeleton's joints, carried on the loaded model beside the existing document-level tags. Per-node keys are a separate namespace from the document-level `tags` key. Same degradation contract as document extras: any malformed value yields no zone for that joint, never an error. Tests cover tagged, untagged, malformed, and leaf-joint cases.

### Task 2: World-joint sampling API

Add a public sampling entry to the model animation module that produces **world-space joint transforms** (one per joint, pre-inverse-bind) for the same inputs the palette samplers take — single clip with loop policy, or a two-source blend at a weight. Reuses the existing forward-sweep compose; the skinning-palette functions are untouched. Allocation contract matches them (caller-reused output buffer, scratch reuse).

### Task 3: Game-side model store

Retain each uploaded model's CPU skeleton, clips, and Task 1's zone table in a game-layer store keyed by model handle, populated at the level-load model sweep beside the existing clip-table install and cleared on level change with it. Share the underlying data with the renderer's copy via `Arc` rather than cloning clip keyframes. This is the visibility-independent pose source; nothing renderer-side is queried per shot.

### Task 4: Posed-capsule raycast + weapon integration

A hit-zone query: for a candidate entity, resolve its current animation state to sample inputs (the game side's existing state → clip/time/fade resolution), sample world joints via Task 2, build capsules per zone-tagged joint (segment to first child, sphere for leaves, model→world via the entity transform), and ray-test each (parry3d). Integrate into the weapon's entity-hit walk: AABB slab test stays as broad phase; on a broad-phase pass for a zone-bearing model the capsule test decides hit, point, normal, and zone; entities without zones keep the AABB result. The weapon impact gains an optional zone tag, populated only for capsule hits. Plumbing: the weapon tick gains access to the Task 3 store and the animation clock through its caller (the app's weapon-fire tick already borrows the registry; the store and clock ride the same call).

### Task 5: Zone multipliers in the descriptor + damage application

`zoneMultipliers` (tag → factor map, optional) on the health descriptor: Rust struct + validation (factors finite and ≥ 0), both JS and Luau parse arms, typedef generation, hot-reload refresh, and materialization onto the health component. At the app's damage-routing site, scale the payload amount by the target's multiplier for the impact's zone tag (absent zone or absent table → 1.0) before the damage chokepoint. Level-load validation cross-checks declared tags against the spawned model's zone table and warns once per archetype per unknown tag.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — loader and sampler, disjoint files.
**Phase 2 (sequential):** Task 3 — consumes Task 1's zone table type.
**Phase 3 (sequential):** Task 4 — consumes Tasks 2 + 3; touches the weapon module and the app tick.
**Phase 4 (sequential):** Task 5 — consumes Task 4's zone-on-impact; touches the same app damage-routing site Task 4's caller plumbing lands in.

## Rough sketch

- Loader: `model/gltf_loader.rs` (per-node extras beside `read_model_tags`); zone table on `LoadedModel` (e.g. `joint_zones: Vec<Option<JointZone>>`, parallel to `Skeleton::joints`). File is large but test-dominated; extend in place, no split.
- Sampler: `model/anim.rs` — a world-joint sibling of `sample_clip_looped` / `sample_blended` sharing `compose_palette`'s sweep (factor the sweep so one core serves both outputs).
- Store: new `scripting/systems/hit_zones.rs` owning the model store + query, installed in `main.rs` at the same sweep that fills `MeshClipTables` (`MeshClipTables::insert` call site); state→sample-input resolution reuses `scripting/systems/mesh_anim.rs` logic against `MeshAnimation` (`scripting/components/mesh.rs`).
- Weapon: `weapon/mod.rs` `nearest_entity_hit` grows the zone path; `WeaponImpact` gains `zone: Option<String>`; ray-vs-capsule via `parry3d` query against `parry3d::shape::Capsule` (Y-axis convention already used by movement).
- Descriptor: `scripting/data_descriptors.rs` (`HealthDescriptor.zone_multipliers`), `components/health.rs` materialization, `typedef.rs`, hot-reload refresh path; damage application at the `apply_damage` call in the app's weapon-fire tick (`main.rs`).
- Oversized-file flag: `main.rs` is far past the soft threshold; this plan adds only call-site lines there (store install, clock/store args, one multiplier lookup) — no split in this plan, flag carried for a future refactor.
- Engine default capsule radius when `hitZoneRadius` is absent: 0.12 m.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | glTF extras |
|---|---|---|---|---|---|
| zone multipliers | `zone_multipliers` | `"zoneMultipliers"` (descriptor JSON) | `zoneMultipliers` | `zoneMultipliers` | n/a |
| zone tag | `String` (verbatim) | verbatim | verbatim | verbatim | `"hitZone"` value |
| zone radius | `radius: f32` | n/a | n/a | n/a | `"hitZoneRadius"` |

Zone tags are author-defined strings compared verbatim across all boundaries; no casing transform.

## Open questions

- Whether the broad-phase AABB should auto-inflate to bound the posed skeleton (a capsule can swing outside the authored hitbox). v1: authors size the hitbox to bound the animation set; level-load validation is not attempted. Revisit with real enemy assets.
- `Arc` granularity in the shared model data (whole model vs. per-part) — implementation detail at code contact; the constraint is no per-shot deep clone of clip keyframes.
