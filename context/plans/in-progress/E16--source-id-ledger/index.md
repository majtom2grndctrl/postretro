# E16 - Source ID Ledger

> **Status:** ready - reviewed, awaiting implementation.
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
contributor ledger until it despawns or the level reloads.

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
- A bounded per-target contributor ledger stored directly in the target's
  `HealthComponent`.
- Contributor recording happens only inside the damage chokepoint. Clearing is
  automatic: entity despawn drops the component, and level reload rebuilds the
  registry.
- Death sweep snapshots ledger facts before the despawn or death latch; clearing
  then follows automatically from the despawn (component drop).
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
- A first-contribution timestamp for downstream `timeToKill` — deferred to the
  `onKill` spec, which threads a clock and consumes it; the in-memory ledger
  makes it cheap to add then.

## Acceptance criteria

- [ ] Authors can set `components.weapon.creditSource` in TypeScript and Luau.
  Missing `creditSource` yields a stable default; an invalid value is rejected
  through the existing weapon-descriptor `validate()` path (as other invalid
  weapon fields are today), identically in both runtimes.
- [ ] Generated TypeScript and Luau SDK types include `creditSource` on
  `WeaponDescriptor` with the same camelCase spelling.
- [ ] Adding `creditSource` preserves live cooldown/trigger state across hot
  reload (through the existing `refresh_from_descriptor` path). A
  canonical-defaulted `creditSource` also survives hot reload — an
  implementation that regresses it to `weapon.unknown` on reload fails this
  criterion.
- [ ] Hitscan weapon damage records a contributor entry on the struck target
  using the weapon's effective source id. Hit-zone multiplier behavior stays
  unchanged.
- [ ] `applyDamage` reaction damage records an environmental/script contributor
  entry rather than looking like weapon damage.
- [ ] Enemy AI attack damage records an enemy-attack contributor entry and still
  damages the player through the existing health path.
- [ ] Damage applied to a target after it is death-latched
  (`HealthComponent::death_handled`) is not recorded into the ledger.
- [ ] Repeated damage from the same source aggregates into one ledger entry with
  accumulated damage and last-hit metadata.
- [ ] More distinct source ids than the configured ledger capacity cannot grow
  unbounded memory. The retained entries are deterministic and keep total
  damage accountable through an overflow bucket or equivalent reduced entry.
- [ ] A death sweep report contains a snapshot of the killed target's ledger
  facts before the target is despawned or death-latched.
- [ ] Ledger state is gone once the target despawns (component drop) or the
  level reloads. In-place respawn or checkpoint reset — same `HealthComponent`,
  HP restored — is deferred to a future respawn spec; the engine ships no such
  path today.
- [ ] No new combat reward, score, XP, damage-number, or grant behavior runs as
  part of this plan (review/grep gate, not a runnable assertion).
- [ ] No new `unsafe` is introduced (review/grep gate, not a runnable assertion).

## Tasks

### Task 1: Descriptor and effective source id

Add `creditSource` to `WeaponDescriptor` and `WeaponComponent`, preserving live
cooldown/trigger state on hot reload. Validate it as a non-empty ASCII
identifier matching `[A-Za-z0-9_.:-]`, max 64 bytes — a charset that stays safe
later as a categorical predicate value and a per-key store-slot segment. Parse
it through both descriptor runtimes, add it to typedef registration, and update
generated SDK fixtures.

Default policy: authored `creditSource` wins when present; otherwise the
resolved canonical equip/spawn name; otherwise the literal `weapon.unknown`
(warn once in debug builds). The default resolves at spawn/materialization — a
missing field parses to `None`, and the spawn path fills the default. The
engine-derived default is engine-provided and is not re-validated against the
`creditSource` charset.

The spawn path must know the descriptor's canonical equip name to provide the
default. If the current materialization path only hands `WeaponDescriptor` to
`WeaponComponent::from_descriptor`, add a small constructor input or spawn-time
wrapper that supplies the resolved canonical name. Do not infer the default from
an entity id or display name.

Hot reload must not regress a canonical-defaulted source: `refresh_from_descriptor`
receives only `&WeaponDescriptor`, so when an authored `creditSource` is absent
on reload, refresh must retain the previously-resolved `credit_source` (or
thread the canonical name into the refresh path) rather than falling back to
`weapon.unknown`.

`credit_source` surfaces through `WeaponComponent::effective()` / `EffectiveStats`
(matching the Boundary inventory's `EffectiveStats::credit_source`), so
producers read the effective source id, not a raw component field.

`WeaponDescriptor` is not `Default`, so adding `credit_source` requires
updating every `WeaponDescriptor` struct literal, production and tests;
consider adding a constructor or `Default` impl to bound the churn.

### Task 2: Damage context and chokepoint recording

Introduce `DamageContext` in the `postretro-entities` crate, next to
`apply_damage` — it references `EntityId`, so the §12 partition rule keeps it
out of `postretro-foundation`. Fields: `source_id`, `attacker: Option<EntityId>`,
`weapon: Option<EntityId>`, `zone: Option<String>`.

Add a context-taking chokepoint entry (e.g. `apply_damage_with_context`) and
keep the existing `apply_damage` as a thin shim that forwards an
unattributed/empty context, so every current production caller (`sim/mod.rs`,
`health/reactions.rs`, `scripting/systems/ai.rs`) and test caller keeps
compiling at this phase boundary. Task 4 migrates the production producers to
the context-taking entry with real contexts; the shim is removed once the last
producer has migrated.

The chokepoint mutates HP exactly as today, then records into the target's
ledger when the target has health, is not already death-latched
(`HealthComponent::death_handled`), and the damage amount is positive and
finite. The latch gate stops a latched player or brain enemy from accumulating
further contributions.
Entities without health still ignore damage. Invalid amounts keep the current
producer-side warn/no-op behavior.

### Task 3: Health-owned bounded ledger

Add a contributor ledger stored directly in `HealthComponent`. The engine has
no component save/load and replication is state-slot-schema-based, so transient
combat history costs nothing there; in-component storage also clears the ledger
automatically on component drop.

Ledger entry facts for this slice: source id, accumulated post-mitigation
damage, hit count, last-hit damage, last-hit zone when known, last attacker id
when known, and last weapon id when known. The source id is the durable
attribution key; the attacker and weapon `EntityId`s are best-effort convenience
and may be stale by kill time (entities despawn — callers must not hold ids
across destruction). Downstream categorical facts, such as last-hit weapon
identity, derive from the stable source id, not the `EntityId`.

Adding a field to `HealthComponent` requires updating every exhaustive struct
literal and destructuring pattern across the crate, plus the `from_descriptor`
initializer; consider initializing the ledger via a default to bound the churn.

Expose the recording entry point that encapsulates capacity and overflow;
Task 2's chokepoint calls it under its gate and does not reimplement insertion.
Pin a small capacity constant. Retained source ids stay exact — never mutate
one retained source id into another. When capacity is exceeded, overflow
collapses into a separate reduced entry that preserves total recorded damage.
The overflow design should not make a later `damageBy(source)` fact lie for
retained source ids.

### Task 4: Wire all damage producers

Weapon hitscan, `applyDamage`, and enemy AI attacks must build explicit
contexts. `WeaponImpact` carries only `target` and `zone` — weapon hitscan
takes its source id from the active wieldable's `WeaponComponent` (via
`effective()`), the weapon entity id from that same active wieldable entity,
and only target and hit-zone tag from `WeaponImpact`. `applyDamage` uses a
fixed script source such as `script.applyDamage`. Enemy AI uses a fixed source
such as `enemy.attack` plus attacker id from the brain entity.

The three apply sites: weapon damage in `sim/mod.rs::run_weapon_fire_tick`, the
`applyDamage` reaction in `health/reactions.rs`, and enemy AI in
`scripting/systems/ai.rs` (via `run_ai_tick_with_navigation`; the attacker id
is the brain entity / `outcome.id`).

Keep zone-multiplier scaling at the weapon damage site before the payload
reaches the chokepoint, so the ledger records the post-mitigation payload the
chokepoint receives. That amount is unclamped: an overkill blow records the full
post-mitigation payload, not the remaining HP.

### Task 5: Death-report snapshot and clearing

Extend `DeathReport` (currently `killed_tags: Vec<Vec<String>>` plus
`player_died: bool` — no `EntityId` crosses the sweep boundary) with two new
fields: a `killed_contributor_ledgers: Vec<ContributorLedgerSnapshot>`,
index-aligned with `killed_tags`, and a `player_contributor_ledger:
Option<ContributorLedgerSnapshot>`, parallel to `player_died`, each carrying
the per-source entry facts. `ContributorLedgerSnapshot` is a plain clone of the
per-source ledger entry facts from Task 3, not a distinct reduced type. The
sweep must capture ledger facts before the
pass-2 despawn/latch writes, since the entity ids and components are gone once
the sweep returns. The progress tracker still receives tags as today; later
combat-event specs consume the new snapshots.

Brain enemies keep their ledger through the death latch only until the snapshot
is captured; the `death_handled` gate from Task 2 stops further accumulation,
and they must not re-report while waiting for animation despawn. Despawn then
drops the component and its ledger. In-place respawn reset is deferred to a
future respawn spec.

### Task 6: Tests and docs

Add focused Rust tests for descriptor parsing, effective source defaults,
chokepoint ledger aggregation, bounded capacity, producer contexts, death-report
snapshots, and clearing. Confirm generated SDK type snapshots are current —
Task 1 already regenerates the fixtures, since the Phase-1 `cargo test` drift
check requires it. `docs/scripting-reference.md` has no
`components.weapon` section yet (only `components.health`); create the weapon
descriptor surface section, mirroring `## components.health`, then add the
`creditSource` note there.

## Sequencing

**Phase 1 (sequential):** Task 1, then Task 3 - descriptor/default source and
the health ledger both edit shared struct-literal call sites, so they run in
sequence.
**Phase 2 (sequential):** Task 2 - consumes the ledger owner and defines the
extended chokepoint.
**Phase 3 (sequential):** Task 4 - rewires all producers to the new chokepoint.
**Phase 4 (sequential):** Task 5 - consumes recorded ledger data in the death
sweep.
**Phase 5 (sequential):** Task 6 - verifies and documents the completed
surface.

## Rough sketch

Grounded identifiers: `DamagePayload` in
`crates/foundation/src/foundation_pods.rs` (re-exported through
`weapon/damage.rs`); `ActivationOutcome::Hit(DamagePayload)` and `WeaponImpact`
(carrying `target: Option<EntityId>` and `zone: Option<String>`) in
`weapon/mod.rs`; `WeaponComponent::effective()` returning `EffectiveStats` in
`crates/entities/src/components/weapon.rs`; `WeaponDescriptor` in
`crates/foundation/src/data_descriptors/types/combat.rs`; descriptor parsers in
`crates/scripting-core/src/data_descriptors/js/entity.rs` and
`crates/scripting-core/src/data_descriptors/lua/entity.rs`; the SDK type
registry in `scripting/primitives/mod.rs`; the health chokepoint
`crates/entities/src/components/health.rs::apply_damage`; death sweep
`scripting/systems/health.rs::sweep_deaths`; sim weapon damage in
`sim/mod.rs::run_weapon_fire_tick`; enemy AI damage in
`scripting/systems/ai.rs::run_ai_tick` (the real `apply_damage` call site is
`run_ai_tick_with_navigation`); and the `applyDamage` reaction handler in
`crates/postretro/src/health/reactions.rs` (aliased as
`scripting::reactions::apply_damage`).

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
payload. Because `DamageContext` references `EntityId`, it lives in
`postretro-entities` (next to `apply_damage`), not in `postretro-foundation`
where the amount-only `DamagePayload` POD sits — the §12 partition rule keeps
`EntityId`-referencing types in the entities crate.

Default source policy: authored `creditSource` wins; otherwise use the
canonical name used to equip or spawn the weapon instance. If that name is not
available, use a fixed fallback such as `weapon.unknown` and warn once in debug
builds. The engine-derived default (canonical name, or the `weapon.unknown`
fallback) is engine-provided and is not re-validated against the author
`creditSource` charset.

Capacity policy: use a named constant, keep retained source ids exact, and store
overflow as a separate reduced entry. Do not mutate one retained source id into
another.

File-size notes:

- `weapon/mod.rs` is ~277 production lines (the remaining ~720 are tests), and
  this plan does not extend its production code, so no split is needed.
- `main.rs` is ~6,946 lines, but this plan should avoid extending it directly.
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

- `applyDamage` will not gain an optional authored source id here. Attribution
  is modder-owned in the long run, so an authored source eventually exists — but
  it lands with the combat-handler authoring API, which this plan lists out of
  scope. Until then the fixed `script.applyDamage` is a deliberate placeholder,
  not the endpoint.
