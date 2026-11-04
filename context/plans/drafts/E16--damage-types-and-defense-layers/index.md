# E16--damage-types-and-defense-layers

Brief · resumable · Epic 16 · reads: `context/lib/scripting.md` §10.5, §11, §12 · `context/lib/entity_model.md` §1, §Components · `context/lib/networking.md` §What gates, and what replicates instead, §Version gates · read at 44881f79

## Problem
The owner wants to prototype Borderlands-style combat: shields that soak damage and recharge after a delay, armor that resists some damage types and is weak to others, and gear or leveling that changes how a defense performs. None of it can be authored today. Two things block it:
- **The damage chokepoint goes straight to health.** It subtracts health first and only then fires impact policies, so a defense can only refund health after a hit, never absorb it before.
- **Hits carry no damage type.** The damage payload holds an amount and an impulse, nothing else.

When this is done, a mod does three things:
- **Declares the vocabulary.** It names its damage types and defense layer kinds.
- **Stacks layers on descriptors.** Each descriptor stacks layers that drain before health on every damage source, scaled per damage type.
- **Authors recharge.** Each layer's capacity and recharge are literals or expressions over the mod's state.

HUDs, the enemy overlay, impact policies and enemy brains can all read the result.

## Decisions
- **Defense layers are an engine-owned noun with author-named instances.** The engine owns each layer's current value and its drain and recharge system. Authors name the layer kinds mod-wide and attach them to descriptors.
  - `entity_model.md` §1 already lists shields as an engine noun. `scripting.md` §11 says "the engine ships the component and its per-tick system; the author ships the policy."
  - This diverges from the roadmap's "author state (per-entity `@state`) + IR policy", deliberately. Per-entity state has no schema, reads a typo as 0, doesn't replicate and has no per-tick writer, which makes it the wrong home for a core resource (`research.md` §Layer home).
  - Author `@state` shields keep working where they're read today (the overlay). The engine never drains them.
- **Mod-wide vocabularies follow factions.** `defineDefenseLayer(name)` and `defineDamageType(name, { against })` handles go in `defineMod({ defenseLayers, damageTypes })`.
  - Components reference both by name.
  - Load fails, naming the key path, on: an undeclared name, a duplicate, or a layer declared with the reserved name `health`.
  - The `health` block now rejects unknown keys; today it silently ignores them.
- **Layers live inside `health`, in drain order.** Each entry is `{ layer, capacity, rechargeDelayMs?, rechargeRate? }`.
  - Omitted, `rechargeDelayMs` is 3000 and `rechargeRate` is 0. Rate 0 means the layer never recharges, which is armor.
  - Literal ranges: capacity [0, 1e6], delay [0, 60000] ms, rate [0, 1e6] points/s. A literal outside its range rejects load.
- **A layer parameter is a literal or a Number expression, so it changes by derivation, not by writes.** This follows `assistable.radius` / `holdMs` (`plans/done/E18--player-revive-assist`).
  - Leveling and gear raise a store slot, and the expression reads it. Persistence and co-op stay single-source, and no write verb exists.
  - The expression scope reads global store values, the entity's own `@state.*`, and (on the player entity only) the pawn's engine per-player slots and its seat's per-owner store slots, as plain reads. A per-owner read on any other descriptor rejects load.
  - Expressions are evaluated each tick, on the host, for entities that carry layers. Results clamp into the literal ranges. A fault keeps the last good value and warns once per descriptor and field.
  - Lowering capacity clamps the current value down. Raising capacity never fills the layer; recharge does.
- **A mod-wide matrix plus per-descriptor resistances decide how much a typed hit drains.** `defineDamageType(name, { against: { <layer kind> | health: m } })` sets how much one raw point of that type takes from each pool. A missing entry is 1. `health: { resist: { <type>: m } }` multiplies every pool for that type on that entity. Multipliers fall in [0, 64].
  - Untyped damage is ×1 everywhere, so a mod without types or layers behaves exactly as it does today.
  - Type identity never enters the IR as a value. Policies read a Bool per type (`scripting.md` §11: "prefer a field on the named thing"), so this brief does not wait on interning.
- **Drain order and spillover are engine rules (owner: every source drains, Borderlands lean).** All of this happens in the damage chokepoint, for every producer.
  - The incoming amount (after hit zones, as today) meets each layer in order. A layer above zero takes `raw × m` points.
  - When a layer empties, only the raw remainder it could not absorb moves on to the next layer. A shock bonus against shields therefore never carries over to flesh.
  - A multiplier of 0 means the type bypasses that layer without touching it. A zero-valued layer is skipped. Health takes the remainder × its multiplier.
  - The death latch, the sweep and `target_was_live` keep reading health only.
- **Every producer carries a type.**
  - Weapons carry a weapon-level `damageType`, which covers hitscan, projectiles, splash, NPC ranged attacks and traps.
  - NPC melee attacks carry `attacks.<name>.damageType`.
  - Movers carry the FGD KVP `crush_damage_type`.
  - The script verb is `.damage(n, { type })`.
  - Unknown names are handled per surface:
    - In descriptors, they fail load.
    - In FGD, they warn once at level load and apply untyped, because level content is not mod vocabulary.
    - In a reaction, the reaction is dropped at install with a log naming it, following the existing install-drop precedent.
- **Recharge is an engine per-tick host step at tick start.** It runs after the respawn drain, so every hit in tick N lands after that tick's recharge.
  - The chokepoint stamps the host tick of the last hit. Any hit with a positive incoming amount resets every one of the entity's layer delays, even a hit that was fully absorbed or bypassed.
  - Recharge pauses while the death latch is set. Respawn refills every layer to capacity. `setHealth` and `grantHealth` leave layers alone.
- **Layer values follow health's lifecycle.**
  - Spawn fills every layer.
  - Level change, same-map restart and rejoin carry each value beside carried health, clamped to the new capacity.
  - Hot reload clamps kept layers, fills added ones and drops removed ones.
- **Impact facts keep their meaning and gain new ones.**
  - Unchanged: `@impact.amount` stays the incoming amount before type and layers, and `healthBefore` / `healthAfter` stay health-only. Mods, the `coop--predicted-combat-feedback` draft's prediction rule and the `E10--enemy-stagger` draft's accumulator keep the meaning they rely on.
  - New, all in pool points: `absorbed` (total drained from layers by this hit), `broke` (this hit took some layer from above zero to zero) and a Bool per declared type.
  - The target's layer values and maxes are readable after the hit. This is a deviation: the session handoff recommended changing `@impact.amount` to mean post-mitigation damage. Keeping it as incoming damage protects existing readers at no cost to expressiveness.
- **Credit and threat count points dealt.** The contributor ledger and kill credit, and brain threat, record the points dealt across layers and health together. A player who only stripped a shield is a contributor. Brain damage recency resets on any hit with a positive incoming amount.
- **One read surface serves the HUD, the overlay and AI.**
  - A layer handle's `.value` / `.max` reads the evaluated entity: the impact target, the brain's own enemy, or the overlay entity.
  - In HUD, store and crossing reads it means the local player. It takes `byPlayer` wherever per-player slots do.
  - For the player, it is a new owner-private engine slot family keyed by declared layer name. It reads the pawn's layer through the one pawn-to-value lookup (`scripting.md` §5, Per-player engine slots), and is absent for a pawn without that layer.
  - This changes the state-schema fingerprint, not `WIRE_VERSION`, following the owner-private weapon-sample precedent.
  - The overlay scope gains the layer family beside `@state.*`, and "hide at full" counts a layer below capacity as not full.
- **Non-goals:**
  - Status effects and damage over time.
  - Crits.
  - Shield special effects (nova, spike, absorb-to-ammo).
  - Per-action damage types. Weapon-level only; augments may add one later (`research/weapon-model.md` §3).
  - A verb that refills a layer (a shield booster pickup). An additive grant, later.
  - Teammate shield bars. Owner-private values never leave their owner; a teammate HUD is its own replication decision.
  - Client prediction of layer values. Host samples are enough for a bar.
  - Impact policies for script and environmental damage. Roadmap "DoT / environmental death policy" owns that; layers still drain on those paths.
  - Pair-scope layer reads.

### Scripting surface
```ts
// start-script.ts — mod-wide vocabulary
const shield = defineDefenseLayer("shield");
const armor = defineDefenseLayer("armor");
const shock = defineDamageType("shock", { against: { shield: 2, armor: 0.5, health: 0.75 } });
const corrosive = defineDamageType("corrosive", { against: { shield: 0.5, armor: 2 } });
defineMod({ defenseLayers: [shield, armor], damageTypes: [shock, corrosive], /* … */ });

// player.ts — recharge scales with a per-owner progression slot
health: {
  max: 100,
  layers: [{
    layer: "shield",
    capacity: read(progression.shieldCapacity).plus(50),
    rechargeDelayMs: 2500,
    rechargeRate: read(progression.shieldTech).times(4).plus(20),
  }],
},

// reference-enemy.ts — armored, shock-resistant
health: { max: 70, layers: [{ layer: "armor", capacity: 120 }], resist: { shock: 0.5 } },

// weapons, attacks, script damage
{ name: "arc-rifle", damage: 12, damageType: "shock" /* … */ }
attacks: { spit: { damage: 15, damageType: "corrosive", maxRange: 2, cooldownMs: 800 } }
npcs({ tag: "turret" }).damage(40, { type: "shock" });

// reads
hudBar({ value: shield.value, max: shield.max });          // local player
when(impact.broke.and(impact.type(shock)), [/* … */]);     // impact policy
guard: read(shield.value).le(0),                            // enemy brain: own layer
```
Luau mirrors every name: `Postretro.defineDefenseLayer("shield")`, `shield.value`, `impact.type(shock)`, and so on. Both runtimes ship.

## Acceptance

### Declaration and validation
- [ ] A mod that declares layer kinds and damage types loads, in TS and in Luau, producing identical descriptors.
- [ ] Load fails, naming the key path, for each of: an undeclared layer in `layers` or `against`; an undeclared type in `resist`, a weapon or an attack; a duplicate declared name; a layer named `health`; an out-of-range literal; an unknown key inside `health`. A valid sibling of each loads.
- [ ] A layer expression with a per-owner read loads on the player entity and fails load on an enemy descriptor.
- [ ] An unknown FGD `crush_damage_type` warns once at level load and crushes untyped. A script `.damage(n, { type })` with an unknown type is dropped at install with a log naming the reaction.

### Drain
- [ ] A mod with no types or layers: health results and every existing impact fact are identical to before.
- [ ] Ordered spillover: shield 30, armor 50, health 100; an untyped 100 leaves 0 / 0 / 80.
- [ ] Spillover in raw units: shock (shield ×2, health ×0.75) for 20 against shield 30 empties the shield on 15 raw, and health loses 3.75.
- [ ] Multiplier 0 on a layer leaves it untouched and passes the hit on. Multiplier 0 on health deals no health damage. `resist` scales on top, and `resist` 0 makes the entity immune to that type.
- [ ] A fully absorbed hit leaves health unchanged, dispatches the impact with `absorbed` equal to the points drained, and never latches death.
- [ ] A zero-valued layer is skipped and never reports `broke`.
- [ ] Two hits on one tick drain in order: the second sees the first's result, and only the hit that empties a layer reports `broke`.
- [ ] Each producer drains a test shield by its type's multiplier: hitscan, direct projectile, splash, NPC melee, NPC ranged, trap, crush and the script verb.

### Recharge
- [ ] Nothing recharges before the delay. After it, the layer ramps at the rate and stops at capacity.
- [ ] A hit mid-ramp restarts the delay; so does a fully absorbed or bypassed hit. A rate-0 layer never recharges. A delay of 0 starts recharging on the next tick.
- [ ] A hit on the tick the delay expires: that tick's recharge applies first, the hit then drains, and the delay restarts.
- [ ] An expression parameter tracks a store change on the next tick. Lowering capacity clamps the value; raising it does not fill. A faulted expression keeps its last good value and warns once.
- [ ] An entity latched at zero health does not recharge. Respawn refills every layer.

### Lifecycle
- [ ] Level change, same-map restart and rejoin carry a player's layer values, clamped to the new capacity. Spawn fills. Hot reload clamps kept layers, fills added ones and drops removed ones.

### Reads
- [ ] An impact policy reads `absorbed`, `broke`, the per-type Bool, and the target's layer value and max after the hit.
- [ ] An enemy brain guard reads its own layer and changes state when the layer breaks.
- [ ] The overlay's `shield` reads the layer. A shield-only hit at full health keeps the overlay shown.
- [ ] The player's layer value and max read the host value on the host and on a connected client. A pawn without that layer reads absent.
- [ ] A player who only drained an enemy's shield is credited as a contributor when the enemy dies. Brain threat counts points dealt. A fully absorbed hit resets brain damage recency.
- [ ] The Scripting surface example runs as `content/dev` content and loads clean in both runtimes.
- [ ] `docs/scripting-reference.md` documents the vocabulary, the drain rules and a Borderlands recipe. Every command in it runs from an SDK bundle.

### Manual
- [ ] Playtest the triangle in the dev mod: shock strips a shielded enemy, corrosive eats an armored one, each is weak on the other.
- [ ] Player shield feel: delayed recharge, and a progression bump visibly speeds the recharge.
- [ ] HUD shield bar and enemy overlay shield read correctly. A co-op client's HUD bar tracks the host at the lag profile.

## Path
- **Seams:** `apply_damage_with_context` and `DamageContext` (health.rs); `DamagePayload` (foundation_pods.rs); `HealthDescriptor` and its validation (combat.rs); faction drains and resolution (`drain_factions_js`, `resolve_entity_faction_indices`) as the vocabulary precedent; `NumberOrIr` and `AssistPairTable::bind` as the literal-or-expression precedent; `PlayerConditionScope` as the "plain read means this player" precedent for the parameter scope; `IMPACT_DISPATCH_INPUTS` and `EntityScope`; `BrainInputRef::State` and `OverlayStateScope` for the `@layer.` family; `PlayerSlot`, `engine_state_catalog`, `owner_private_source_value` and `MIRRORED_GAME_STATE_LEAVES` for the owner-private family; `CarriedState` and `CarriedField` (the destructure has no wildcard); `respawn::drain` and `begin_tick` for recharge placement.
- **Chosen shape vs the strongest rival.** Chosen: an engine layer stack with an authored matrix and expression parameters. Strongest rival: the same engine storage, drain and last-hit stamp, but recharge (and optionally overflow) authored as an IR expression over a layer-local scope (value, max, time since hit, tick delta). That is `scripting.md` §11's split read literally, and the hot IR write path `plans/done/M14--movement-dash-runtime-values` earmarked shield recharge for. The chosen shape is a subset of it: adding those inputs to the parameter scope, or a per-layer recharge expression beside delay and rate, grows into the rival without breaking a mod. Not now, because it builds the per-entity IR write path before any mod needs more than delay-then-rate. A weaker rival, author `@state` layers, is schema-less, unreplicated and silent on typos.
- **First slice:** one player shield end to end, covering descriptor, drain, literal recharge and the owner-private read on a connected client. That slice falsifies the riskiest pieces: the new owner-private family and the tick-start recharge order.
- **Split first.** These files run past ~800 lines: `impact_policy.rs`, `state_slots.rs`, `scopes.rs`, `combat.rs`, `health.rs`. Split each first where the brief extends it: behavior-preserving, in its own commit.
- **Coordinate with the drafts.** On landing, update the stagger and predicted-feedback drafts with a line on which amount they read.

## Open questions
- Whether layers get a per-tick cost bound (count of layered entities × expression size) beyond the IR's own depth and node caps — **delegated**: measure on the dev mod and report in the plan of record.
- Names of the Rust types and the SDK `hudBar` helper spelling — **delegated**: follow existing SDK UI helpers; the example's `hudBar` stands for whatever binds a bar today.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Layer declaration | executor | `defenseLayers` | `defineDefenseLayer(name)` | `Postretro.defineDefenseLayer(name)` | n/a |
| Damage type declaration | executor | `damageTypes`, `against` | `defineDamageType(name, { against })` | `Postretro.defineDamageType(name, { against = … })` | n/a |
| Descriptor layer entry | executor | `layers[].layer`, `capacity`, `rechargeDelayMs`, `rechargeRate` | same | same | n/a |
| Resistances | executor | `health.resist` | same | same | n/a |
| Weapon / attack type | executor | `damageType` | same | same | n/a |
| Crush type | executor | n/a | n/a | n/a | `crush_damage_type` |
| Script damage type | `ApplyDamageArgs` field | `{ amount, type }` | `.damage(n, { type })` | `:damage(n, { type = … })` | n/a |
| Layer read | IR input | `@layer.<name>`, `@layer.<name>.max` | `handle.value`, `handle.max` | same | n/a |
| Impact facts | IR inputs | `@impact.absorbed`, `@impact.broke`, `@impact.type.<name>` | `impact.absorbed`, `impact.broke`, `impact.type(handle)` | same, `:` calls | n/a |
| Owner-private family | catalog rows per declared layer | slot names `player.layer.<name>`, `player.layer.<name>.max` | via handle | via handle | n/a |
