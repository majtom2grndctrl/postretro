# Research: Skeletal hit zones — pre-spec brief

> Prep for `/draft-spec`. Roadmap task: M10 "Skeletal hit zones" (`plans/roadmap.md:218`).
> Wave context: pairs with `M10--navigation-representation` in the same orchestration wave —
> disjoint subsystems. All dependencies shipped (skinned animation runtime, weapon primitives,
> entity health + damage).

## Scope intent (from roadmap)

Bone-parented proxy capsules posed each frame from the skeleton, raycast separately from the static
collision world (net-new — the weapon hitscans static geometry plus one per-archetype AABB today).
Hit-zone identity from glTF per-node `extras` tags (`head`, `limb`); per-archetype damage multipliers
in the descriptor script. Model ships the spatial tag, script ships the balance.

## Load-bearing finding 1: posed joints are CPU-available, but only renderer-side today

- `model/anim.rs` samplers are pure, allocation-free, and reusable: `sample_clip` (`:188–217`),
  `sample_clip_looped` (`:226–239`), `sample_blended` (`:255–267`) return `BonePaletteEntry` skinning
  matrices; `compose_palette` (`:153–185`) does the parent-before-child sweep producing **world-space
  joint matrices**; `sample_local_trs` (`:316–330`) exposes per-joint local TRS.
- **But** poses are sampled renderer-side for *planned (visible)* instances only. The shipped
  animation plan explicitly deferred a game-side, visibility-independent sampling path to this spec
  (`plans/done/M10--skinned-animation-runtime/` open questions §11). The model module is CPU-only by
  contract, so the game-side path is additive: a new system (suggested
  `scripting/systems/hit_zones.rs`, mirroring `mesh_anim.rs`) calls the samplers directly off the
  same animation clock — **do not introduce a second clock** or zones desync from visuals.
- Animation state per entity: `MeshAnimation` in `scripting/components/mesh.rs:110–144`
  (`current_state`, state map, crossfade bookkeeping); sample-time resolution in
  `scripting/systems/mesh_anim.rs`.

## Load-bearing finding 2: glTF extras are document-level only today

- `model/gltf_loader.rs:162–186`: `read_model_tags` reads root-document `extras.tags` →
  `LoadedModel::tags`; malformed extras degrade to empty, never fail load. **Per-node extras are not
  read.** The mesh-loading plan reserved a distinct per-node namespace for hit zones (e.g.
  `extras.hitZone = "head"`) to avoid colliding with model-level `tags`.
- Loader extension: read each joint node's `extras`, carry a per-joint zone tag map alongside (or on)
  `Skeleton` (`model/skeleton.rs`: `Joint` has parent index, `rest_local`, `inverse_bind`).

## Weapon hitscan integration

- Fire path: `weapon/mod.rs` — `tick` (`:70–118`) → `fire_hitscan` (`:120–183`). World hit via
  `cast_ray` (`collision/mod.rs:162–172`, parry3d over the static `TriMesh`); entity hit via
  `nearest_entity_hit` (`weapon/mod.rs:196+`), currently a **ray-vs-world-aligned-AABB** test over
  entities with `HealthComponent::hitbox`. Nearest `toi` wins between world and entity.
- `WeaponImpact` (`weapon/mod.rs:38–49`): `point`, `normal`, `target: Option<EntityId>`,
  `outcome: ActivationOutcome::Hit(DamagePayload)`. **Spatial info rides beside the payload, never
  inside it** — deliberate, so a zone multiplier applies post-resolution without changing
  `DamagePayload { amount }`.
- Insertion choice: replace/augment `nearest_entity_hit` with posed-capsule raycast (per zone), return
  the hit zone alongside the target, scale `payload.amount` by the archetype multiplier before
  `apply_damage` (`scripting/systems/health.rs:87–96` — the single damage chokepoint).

## Collision primitives

- parry3d is the single collision dep. `Capsule` (Y-axis convention) already used for the player
  sweep (`collision/mod.rs:130–156`, `SKIN_DISTANCE = 0.02`). Ray-vs-capsule comes free from parry3d
  query functions — no bespoke math needed. Hit zones should stay **raycast-only** (no movement
  blocking), matching the existing AABB's role.

## Descriptor / SDK side

- Today: `HealthComponent { max, current, hitbox: Option<Hitbox>, death_handled }`
  (`scripting/components/health.rs:23–33`); `Hitbox` is one world-aligned AABB per archetype
  (entity_model.md §7; health-damage plan decisions #5–6 explicitly defer rotation-aware/skeletal
  volumes to this spec).
- Descriptor plumbing for `zoneMultipliers` (e.g. `{ head: 1.5, limb: 0.8 }`): Rust struct +
  validation in `scripting/data_descriptors.rs` (both QuickJS and Luau parse arms), materialization
  in `builtins/data_archetype.rs::attach_descriptor_components`, typedef generation in
  `scripting/typedef.rs`, hot-reload in `refresh_plan.rs`. Examples of the authoring shape:
  `content/dev/scripts/player.ts`, `reference-pistol.ts`, `sdk/behaviors/reference/entities.ts`.
- Open: multipliers inline in the `health` block vs. a sibling `hitZones` block. Note the primitive
  surface is a contract (index.md §2) — SDK types/validation must land in the same pass.

## Open design questions the spec must resolve

1. **Sampling harness**: which entities get game-side poses (all with zones? all with health+mesh?),
   sample cadence (per-frame vs. lazily at raycast time vs. distance-bucketed time-slicing — the
   runtime's time-slicing is designed but unshipped), and storage (per-entity cached capsule buffer
   vs. on-demand).
2. **Capsule authoring**: where radius/half-height per zone come from — glTF per-node extras (shape
   with the model) vs. descriptor (balance with the script) vs. derived from bone length + authored
   radius. Roadmap intent: model ships the spatial tag, script ships the balance.
3. **Fallback semantics**: joint without a zone tag → unhittable, or generic body zone at 1.0×?
   Entity with a mesh but no tagged joints → keep the legacy AABB? (Recommend: AABB remains the
   fallback; zones replace it only when present.)
4. **Multiplier application point**: inside `fire_hitscan` vs. the impact consumer in the tick loop.
5. **Zone vocabulary**: reserved names vs. author-defined strings validated against the model's tags
   at load (warn on descriptor multiplier naming a zone the model lacks).
