# Entity Health + Damage Surface — Research Notes (pre-draft)

Ground-truth inventory gathered ahead of drafting the M10 "Entity health + damage surface" spec (roadmap, Milestone 10 → Combat). Not a spec. Confirm signatures against source before relying on them — read 2026-06-12.

## Roadmap scope (the anchor)

From `context/plans/roadmap.md` (M10 Combat track): a minimal health/damage primitive on the M6 entity model — an entity carries HP, consumes a `DamagePayload`, dies at zero HP. Demonstrated on the enemy (the weapon's target), reused for the player (the enemy's target). Pure M6 — no render, nav, or AI dependency. Shootable as a static proxy, so the shipped weapon gets a target the day it lands. `player.health` slot schema is the **published contract** M13 UI binds against. Keep minimal: representation (dedicated health kind vs. generic scalar-stat kind) is internal; *policy* (regen, recharge, resistances, damage types) defers to Shields + damage-type system over the M14 behavior-IR.

**Execution model:** script declares parameters (a `components.health` block on `defineEntity`, mirroring `movement`/`weapon`), Rust runs damage application and death. No live VM (`scripting.md` §1).

## Published contracts this plan must honor

- **`player.health` / `player.ammo` slots.** `SlotTable::new()` registers both at construction: `SlotType::Number`, `SlotOwnership::Engine`, `readonly: true`, `default: None`, `value: None` (pinned by test `new_registers_engine_player_namespace`, `scripting/slot_table.rs:392-402`). Engine writes go through `write_store_slot` (`scripting/primitives/store.rs:323`) which bypasses readonly but still validates. M13 Goal C bound HUD text to these by dotted name.
- **`DamagePayload` is a struct, never a bare scalar** (`weapon-model.md` §7 #3). Today: `DamagePayload { amount: f32 }`, `weapon/mod.rs:19-22`. Damage types/crit/falloff grow inside it later — consumers must not assume amount-only.
- **`ActivationOutcome` is the named outcome seam** (`weapon-model.md` §7 #8): `Hit(DamagePayload) | Effect | Spawned(EntityId)`, `weapon/mod.rs:24-30`. The health consumer reads `Hit`; the other variants stay declared-but-unreachable.
- **Destruction is immediate** (`entity_model.md` §3): `EntityRegistry::despawn` clears all component cells, bumps/retires generation in the same call (`scripting/registry.rs:619-650`). Callers must not hold `EntityId`s across despawn points.
- **Tuning params are descriptor-owned, never FGD KVPs** (`entity_model.md` §4): max HP and any damage multipliers are not map-overridable.

## What exists (reuse)

- **Weapon fire path.** `weapon::tick` (`weapon/mod.rs`) gates cooldown/fire-mode, calls `fire_hitscan` (`weapon/mod.rs:115-149`) → `cast_ray` against `CollisionWorld` (static trimesh only), returns events with `impact: Option<WeaponImpact>` where `WeaponImpact { point: Vec3, normal: Vec3, outcome: ActivationOutcome }` (`weapon/mod.rs:40-45`). Caller at `main.rs:2634-2637` consumes `impact.point`/`impact.normal` for the particle burst via `spawn_impact_effect_at` (`weapon/impact.rs:38-53`) and **drops the `DamagePayload`** — the consumer seam this plan fills. Test `hitscan_world_hit_returns_impact_point_normal_and_damage_payload` (`weapon/mod.rs:325-349`) pins the payload emission.
- **Component machinery.** `ComponentKind` `#[repr(u16)]`, 10 variants (Transform=0 … DescriptorProvenance=8, Mesh=9), exhaustive `VARIANTS` array backing `COUNT` (`scripting/registry.rs:87-122`). Storage: `[Vec<Option<ComponentValue>>; ComponentKind::COUNT]` dense columns (`registry.rs:424-448`). New-component checklist: struct + `ComponentKind` variant (next discriminant 10) + `ComponentValue` variant + `Component` trait impl (`KIND` / `from_value` / `into_value`) + descriptor field + materialization at spawn.
- **Component precedent to mirror: `WeaponComponent`** (`scripting/components/weapon.rs:20-65`): `from_descriptor()`, `effective()` accessor (identity passthrough — the stats seam), `refresh_from_descriptor()` (hot reload). ~60 lines.
- **Descriptor surface.** `EntityTypeDescriptor` with `components: { movement, weapon, mesh, light, emitter, … }` blocks, `canonicalName`, `defaultWeapon` (`scripting/data_descriptors.rs`); SDK `defineEntity()` is a pure builder (`sdk/lib/data_script.ts:86-91`). Typedefs via `gen-script-types` + drift test; new Rust types must be added to the generator's type-name mapping by hand.
- **Map placement.** Data-archetype dispatch spawns descriptors with a placeable component by `canonicalName` (`scripting/builtins/data_archetype.rs`); `anim_demo_grunt` (`content/dev/scripts/anim-demo-grunt.ts`) is a live map-placeable mesh archetype — the natural shootable-target demo. `spawn_from_player_starts` (`data_archetype.rs:486-550`) materializes the player + default weapon, stores `App::active_wieldable: Option<EntityId>` (`main.rs:589-596`).
- **Entity-entity collision design intent** (`entity_model.md` §7): simple per-entity-type bounding volumes (AABB or sphere), fixed per type, direct geometric checks, no spatial partitioning. **Nothing implemented** — design doc only.
- **Reaction/event surface.** `fire_named_event_with_sequences(event_name, …)` (`scripting/reaction_dispatch.rs:112-139`) dispatches level-declared reactions by event name; tag-targeted primitives resolve targets by entity tag. **`ProgressTracker::on_entity_killed(tags) -> Vec<String>`** (`reaction_dispatch.rs:14-90`) exists for kill-count threshold reactions and **has no caller** — the death path is its intended producer. There is no ID-scoped subscription; everything is name/tag-scoped.
- **Static UI proxy.** `StaticUiProxy` (`scripting/systems/ui_proxy.rs`) writes `DEMO_HEALTH = 100.0` → `player.health` and `DEMO_AMMO = 50.0` → `player.ammo` every frame via `write_store_slot`; file header says "until M10 entity-health replaces it." It also animates `intro.flashColor` (M13 demo) — that part is not health's to remove.
- **Update order** (`main.rs` tick, `entity_model.md` §5): transform snapshot → player movement → camera follow → weapon fire tick → scripting bridges. Events drain after the tick loop.
- **Test precedents.** Weapon tick tests (`weapon/mod.rs:151-406` — spawn, attach component, tick, assert), registry spawn/despawn/generation tests (`registry.rs:811+`), store clamp/readonly/parity tests (`primitives/store.rs:760+`), two-pass collect-then-despawn pattern (`scripting/systems/particle_sim.rs:25-142`).

## Gaps (must build)

- **No health component** — no kind, no descriptor block, no materialization.
- **No ray-vs-entity targeting.** `cast_ray` tests static world only; the weapon plan explicitly deferred entity-volume testing here. Needs an entity hit test along the aim ray (per-type AABB/sphere from §7 design) and nearest-of(world-hit, entity-hit) resolution so walls still block shots.
- **No `Hit(DamagePayload)` consumer** — the payload is dropped at `main.rs:2634-2637` today.
- **No death path.** Nothing calls `despawn` from game logic on a damage condition; `ProgressTracker::on_entity_killed` is never invoked; no `died`/`damaged` named events exist.
- **No real `player.health` producer.** The proxy writes a constant; the plan replaces the `player.*` writes (not the `intro.flashColor` part) with values from the player entity's health.
- **No damage source for the player side.** Enemy AI is a later plan, so demonstrating "player takes damage" needs a stand-in producer (see forks).

## Forks the draft session must resolve

1. **Representation:** dedicated `Health` component kind vs. generic scalar-stat kind (shields later generalize). `entity_model.md` §1 says internal choice, invisible to scripts; roadmap leans "keep minimal."
2. **Entity hit-volume source:** where does a shootable entity's volume come from — a descriptor field, a fixed constant per archetype, or derived (e.g. mesh AABB)? §7 says fixed per entity type. Skeletal hit zones (bone capsules) are explicitly a *later* M10 plan — don't pre-build.
3. **Player death semantics:** despawning the player pawn breaks movement/camera (camera falls back to fly-cam) — likely clamp at 0 + named event, respawn deferred. Needs a decision, not an accident.
4. **Damage/death events:** which named events fire (`damaged`? `died`?), and whether death routes through `ProgressTracker::on_entity_killed` (it should — it's the built waiting consumer).
5. **Player-side damage stand-in:** a debug damage volume, a reaction primitive (`applyDamage` tag-targeted), or defer the player demonstration to enemy AI. Note the roadmap M6 claim that a `DamageSource` reference script shipped is **stale** — `content/dev/scripts/` has no such script (confirmed; weapon research noted the same).
6. **`player.health` schema evolution:** roadmap calls the contract "typed, **ranged**, readonly" but the shipped slot has `range: None, default: None`. Adding a range (0..max) once a real max HP exists is a schema change to a published contract — decide deliberately and update the M13-facing wording either way.
7. **Self-damage / friendly fire / i-frames:** presumably out of scope (policy → behavior-IR), but say so.

## Drift notes (fix or flag while drafting)

- Roadmap M6 marks "Reference behaviors (script) — `RotatorDriver` and `DamageSource`" as shipped; neither exists under `content/dev/scripts/`. Pre-existing drift, also recorded in `done/M10--weapon-primitives/research.md`.
- `entity_model.md` §2's component table omits `DescriptorProvenance` (present in `ComponentKind`). Minor; the table says "current engine components."
- M14 behavior-IR (`ready/M14--behavior-ir-substrate/`, in progress in another session) ships a store-scope **write path with shield-recharge named as a future engine-capability adopter** — health must not pre-build any per-tick policy machinery that plan would own.
