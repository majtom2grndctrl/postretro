# E16 - Source ID Ledger

> **Status:** draft.
>
> **Epic:** 16 - Combat.
>
> **Milestone:** Combat Feedback & Economy.
>
> **Fits first:** front-loads the hard-to-reverse attribution data shape. Later
> `onKill`, `onImpact`, `onDamage`, `CombatScope`, and resource-grant specs
> consume this ledger.

## Goal

Record mod-authored combat attribution at the damage chokepoint. Every damage
application carries a stable source id, and each damaged target keeps a bounded
contributor ledger until death or reset.

This ships no reward policy. It gives later combat-event specs reliable facts
for kill credit, damage buckets, and last-hit attribution.

## Scope

### In scope

- A `creditSource` field on `components.weapon`, parsed in both TypeScript and
  Luau descriptor paths and emitted in generated SDK types.
- Default `creditSource`: the weapon descriptor's canonical equip name when
  known; otherwise a stable engine fallback.
- A damage context passed with every `DamagePayload` application. It carries at
  least source id, attacker id when known, weapon id when known, and hit-zone tag
  when known.
- Weapon hitscan damage stamps the weapon's effective credit source.
- `applyDamage` reaction damage stamps an environmental/script source id.
- Enemy AI attack damage stamps an enemy-attack source id.
- A bounded per-target contributor ledger stored with the target's health state
  or an adjacent health-owned side table.
- Ledger updates happen only inside the damage chokepoint.
- Death sweep snapshots ledger facts before despawn or death latch, then clears
  ledger state for despawned and player-death/reset cases.
- Tests cover descriptor parsing, SDK type generation, weapon damage, reaction
  damage, enemy damage, bounded contributors, death-sweep snapshot, and ledger
  clearing.

### Out of scope

- `onKill`, `onImpact`, or `onDamage` event dispatch.
- Combat handler authoring APIs, `CombatScope`, or behavior-IR bindings.
- XP, score, damage numbers, ammo grants, health grants, or any reward policy.
- Resource-grant chokepoint.
- Ammo, heat, cells, reload, inventory, switching, pickup, augments, alt-fire,
  projectiles, melee as a player action, splash, damage types, crit, shields,
  status effects, or knockback.
- Persisting combat ledgers across save/load.
- Replicating ledgers to clients. The server/host owns combat damage; later
  combat-event specs decide which derived facts replicate or surface.

## Acceptance criteria

- [ ] Authors can set `components.weapon.creditSource` in TypeScript and Luau.
  Missing `creditSource` yields a stable default, and invalid values fail
  descriptor validation.
- [ ] Generated TypeScript and Luau SDK types include `creditSource` on
  `WeaponDescriptor` with the same camelCase spelling.
- [ ] Hitscan weapon damage records a contributor entry on the struck target
  using the weapon's effective source id. Hit-zone multiplier behavior stays
  unchanged.
- [ ] `applyDamage` reaction damage records an environmental/script contributor
  entry rather than looking like weapon damage.
- [ ] Enemy AI attack damage records an enemy-attack contributor entry and still
  damages the player through the existing health path.
- [ ] Repeated damage from the same source aggregates into one ledger entry with
  accumulated damage and last-hit metadata.
- [ ] More distinct source ids than the configured ledger capacity cannot grow
  unbounded memory. The retained entries are deterministic and keep total
  damage accountable through an overflow bucket or equivalent reduced entry.
- [ ] A death sweep report contains a snapshot of the killed target's ledger
  facts before the target is despawned or death-latched.
- [ ] Ledger state is cleared when a target is despawned for death or when a
  player reset lifecycle explicitly starts a fresh health life.
- [ ] No new combat reward, score, XP, damage-number, or grant behavior runs as
  part of this plan.
- [ ] No new `unsafe` is introduced.

## Tasks

### Task 1: Split weapon firing attribution out of `weapon/mod.rs`

Move damage-attribution helpers and impact-to-damage scaling out of the
997-line `weapon/mod.rs` into a smaller module before extending the weapon
path. Keep public `weapon` module exports stable. The split is behavior
preserving and should move tests with the logic where practical.

### Task 2: Descriptor and effective source id

Add `creditSource` to `WeaponDescriptor` and `WeaponComponent`, preserving live
cooldown/trigger state on hot reload. Validate it as a non-empty stable
identifier. Parse it through both descriptor runtimes, add it to typedef
registration, and update generated SDK fixtures.

The spawn path must know the descriptor's canonical equip name to provide the
default. If the current materialization path only hands `WeaponDescriptor` to
`WeaponComponent::from_descriptor`, add a small constructor input or spawn-time
wrapper that supplies the resolved canonical name. Do not infer the default from
an entity id or display name.

### Task 3: Damage context and chokepoint recording

Introduce a damage context type beside `DamagePayload`. Route every producer
through an extended health chokepoint that receives both payload and context.
Keep a compatibility helper only if useful for tests; production damage sites
must pass an explicit context.

The chokepoint mutates HP exactly as today, then records into the target's
ledger when the target has health and the damage amount is positive and finite.
Entities without health still ignore damage. Invalid amounts keep the current
producer-side warn/no-op behavior.

### Task 4: Health-owned bounded ledger

Add a contributor ledger owned by the health subsystem. It may live inside
`HealthComponent` or in a health-owned side table if that keeps component
serialization cleaner.

Ledger entry facts for this slice: source id, accumulated post-mitigation
damage, hit count, last-hit damage, last-hit zone when known, last attacker id
when known, and last weapon id when known.

Pin a small capacity constant. When capacity is exceeded, collapse excess
distinct sources into a deterministic overflow entry or evict by a deterministic
rule while preserving total recorded damage. The overflow design should not
make a later `damageBy(source)` fact lie for retained source ids.

### Task 5: Wire all damage producers

Weapon hitscan, `applyDamage`, and enemy AI attacks must build explicit
contexts. Weapon hitscan uses the effective source id, weapon entity id, target
entity id, and hit-zone tag from `WeaponImpact`. `applyDamage` uses a fixed
script source such as `script.applyDamage`. Enemy AI uses a fixed source such as
`enemy.attack` plus attacker id from the brain entity.

Keep zone-multiplier scaling at the weapon damage site before the payload
reaches the chokepoint, so the ledger records the amount that actually changed
HP.

### Task 6: Death-report snapshot and clearing

Extend `DeathReport` with ledger snapshots parallel to killed entities and
player deaths. The sweep captures ledger facts before despawning plain
non-players and before latching brain/player deaths. The progress tracker still
receives tags as today; later combat-event specs consume the new snapshots.

Clear ledger state when the entity leaves the world or resets to a fresh health
life. Brain enemies keep their ledger after the death latch only long enough for
the death report snapshot; they must not re-report or keep accumulating damage
while waiting for animation despawn.

### Task 7: Tests and docs

Add focused Rust tests for descriptor parsing, effective source defaults,
chokepoint ledger aggregation, bounded capacity, producer contexts, death-report
snapshots, and clearing. Update SDK type snapshots. Add a short
`docs/scripting-reference.md` note for `creditSource` under the weapon
descriptor surface.

## Sequencing

**Phase 1 (sequential):** Task 1 - split before extending the oversized weapon
module.
**Phase 2 (concurrent):** Task 2, Task 4 - descriptor/default source and health
ledger model do not depend on each other.
**Phase 3 (sequential):** Task 3 - consumes the ledger owner and defines the
extended chokepoint.
**Phase 4 (sequential):** Task 5 - rewires all producers to the new chokepoint.
**Phase 5 (sequential):** Task 6 - consumes recorded ledger data in the death
sweep.
**Phase 6 (sequential):** Task 7 - verifies and documents the completed surface.

## Rough sketch

Grounded identifiers: `DamagePayload` in `weapon/damage.rs`;
`ActivationOutcome::Hit(DamagePayload)` and `WeaponImpact` in `weapon/mod.rs`;
`WeaponComponent::effective()` returning `EffectiveStats`; `WeaponDescriptor`
in `scripting/data_descriptors/types/combat.rs`; descriptor parsers in
`scripting/data_descriptors/js/entity.rs` and
`scripting/data_descriptors/lua/entity.rs`; the SDK type registry in
`scripting/primitives/mod.rs`; the health chokepoint
`scripting/components/health.rs::apply_damage`; death sweep
`scripting/systems/health.rs::sweep_deaths`; sim weapon damage in
`sim/mod.rs::run_weapon_fire_tick`; enemy AI damage in
`scripting/systems/ai.rs::run_ai_tick`; and the `applyDamage` reaction in
`scripting/reactions/apply_damage.rs`.

Proposed shape:

```rust
// Proposed design.
pub(crate) struct DamageContext {
    pub(crate) source_id: String,
    pub(crate) attacker: Option<EntityId>,
    pub(crate) weapon: Option<EntityId>,
    pub(crate) zone: Option<String>,
}
```

`DamagePayload` can stay amount-only. The context travels beside it, matching
the existing spatial split where `WeaponImpact` carries target/zone beside the
payload.

Default source policy: authored `creditSource` wins; otherwise use the
canonical name used to equip or spawn the weapon instance. If that name is not
available, use a fixed fallback such as `weapon.unknown` and warn once in debug
builds.

Capacity policy: use a named constant, keep retained source ids exact, and store
overflow as a separate reduced entry. Do not mutate one retained source id into
another.

Split-before-extend:

- `weapon/mod.rs` is 997 lines and this plan adds attribution to its hot path.
  Split first.
- `main.rs` is 6,717 lines, but this plan should avoid extending it directly.
  The relevant simulation seams already live in `sim/mod.rs`.
- `scripting/primitives/mod.rs` is 799 lines. If adding the single
  `creditSource` field pushes it past the threshold, keep the change local.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
| --- | --- | --- | --- | --- | --- |
| Weapon credit source | `WeaponDescriptor::credit_source`, `WeaponComponent::credit_source`, `EffectiveStats::credit_source` | `"creditSource"` in descriptor serde | `components.weapon.creditSource` | `components.weapon.creditSource` | n/a |
| Damage source id | `DamageContext::source_id` | n/a for this plan | Future combat facts read source ids; no handler API here | Future combat facts read source ids; no handler API here | n/a |
| Script damage source | fixed Rust string, e.g. `script.applyDamage` | n/a | `applyDamage` reaction has no new args in this plan | `applyDamage` reaction has no new args in this plan | n/a |
| Enemy attack source | fixed Rust string, e.g. `enemy.attack` | n/a | n/a | n/a | n/a |

## Open questions

- Exact identifier validation for `creditSource`: conservative recommendation is
  non-empty ASCII `[A-Za-z0-9_.:-]`, max 64 bytes. This is enough for
  `weapon.pistol`, `fire`, and `enemy.attack` while staying easy to serialize
  later.
- Whether the ledger lives directly on `HealthComponent` or in a health-owned
  side table. Direct storage is simpler; side table may be cleaner if component
  serialization should not grow with transient combat history.
- Whether `applyDamage` should gain an optional authored source id later. This
  plan deliberately does not add it; the reaction records a fixed script source
  until combat-handler policy needs finer granularity.
