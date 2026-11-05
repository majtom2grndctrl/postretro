# E16--damage-types-and-defense-layers

Brief · resumable · Epic 16 · reads: `context/lib/scripting.md` §1, §10.4, §10.5, §10.8, §11, §12 · `context/lib/entity_model.md` §1, §Components · `context/lib/networking.md` §What gates, and what replicates instead, §Version gates · read at 44881f79

## Problem
The owner wants to prototype Borderlands-style combat: shields that soak damage and recharge after a delay, armor that resists some damage types and is weak to others, and gear or leveling that changes how a defense performs. None of it can be authored today:
- **The damage chokepoint goes straight to health.** It subtracts health first and only then fires impact policies, so a defense can only refund health after a hit, never absorb it before.
- **Hits carry no damage type.** The damage payload holds an amount and an impulse, nothing else.

When this is done, a mod:
- **Declares the vocabulary.** It names its damage types and defense layer kinds.
- **Stacks layers on descriptors.** Each descriptor stacks layers that drain before health on every damage source, scaled per damage type.
- **Authors recharge.** Each layer's capacity and recharge are literals or expressions over the mod's state.

HUDs, the enemy overlay, impact policies and enemy brains can all read the result.

## Decisions
- **Defense layers are an engine-owned noun with author-named instances:** the engine owns each layer's current value and its drain and recharge system, and authors name layer kinds mod-wide and attach them to descriptors. This follows `entity_model.md` §1 (shields are an engine noun) and `scripting.md` §11 ("the engine ships the component and its per-tick system; the author ships the policy"), and deliberately diverges from the roadmap's per-entity `@state` home (`research.md` §Layer home). Author `@state` shields keep working where they're read today (the overlay), and the engine never drains them.
- **Mod-wide vocabularies follow factions:** `defineDefenseLayer(name)` and `defineDamageType(name, { against })` handles go in `defineMod({ defenseLayers, damageTypes })`, and components reference both by name. Load fails, naming the key path, on a duplicate declared name, a layer declared as `health`, a layer kind listed twice in one descriptor's `layers`, or an undeclared layer or type in a descriptor or an IR expression (impact policy, brain guard, overlay, layer parameter); a reaction naming one (`grantLayer`, `.damage`) is dropped at install with a log naming it, following the existing install-drop precedent. The `health` block now rejects unknown keys; today it silently ignores them.
- **Layers live inside `health`, in drain order.** Each entry is `{ layer, capacity, rechargeDelayMs?, rechargeRate? }`; omitted, the delay is 3000 and the rate 0, and rate 0 never recharges, which is armor. Literal ranges are capacity [0, 1e6], delay [0, 60000] ms and rate [0, 1e6] points/s, and a literal outside its range rejects load.
- **A layer parameter is a literal or a Number expression, so it changes by derivation, not by writes.** This follows `assistable.radius` / `holdMs` (`plans/done/E18--player-revive-assist`): leveling and gear raise a store slot the expression reads, so persistence and co-op stay single-source and no write verb exists. The scope reads global store values, the entity's own `@state.*`, and, on the player entity only, its seat's per-owner store slots, as plain reads; a per-owner read on any other descriptor rejects load, and a per-player engine-slot read rejects load on every descriptor.
- **Parameters are host-evaluated, and a store change shows by the next tick.** Results clamp into the literal ranges. Lowering capacity clamps the current value down; raising it never fills the layer on its own.
- **An expression fault, a non-finite result included, keeps that entity's last good value.** This sets a new precedent: assist falls back to 0, and this deliberately diverges because falling to 0 would clamp a live layer to zero. Before any good value, a faulted parameter uses that field's omitted default, or 0 for capacity; the warning fires once per descriptor and field, and hot reload re-arms it.
- **Authors refill layers with an additive grant that matches its siblings exactly:** `grantLayer(layer, amount?)` joins `grantHealth` and `grantAmmo` (`scripting.md` §10.8) through the one grant chokepoint, with a literal finite amount on group and subject tokens and a Number expression on `impact.source`; an omitted amount fills to capacity, a negative amount warns and does nothing, and a recipient without that layer warns and is skipped. A grant evaluates the layer's capacity when it applies, so an impact policy's grant sees that policy's earlier writes, and that evaluation holds from then on, for later hits and as the entity's last good value: the value clamps to it, then the grant adds.
- **A reaction body's store writes, grants and damage land in body order on every source.** This changes engine behavior (`scripting.md` §10.4): on a reaction fired by address (crossings, `levelLoad`, `fire` targets), the host applies store writes (`addSlot`, `setState`) at dispatch, as `grantHealth` and `.damage` do, not at the frame-end drain. Authors read a body top to bottom, so an earlier step is in force for every later one (`research.md` §Body order).
- **A mod-wide matrix plus per-descriptor resistances decide how much a typed hit drains:** `defineDamageType(name, { against: { <layer kind> | health: m } })` sets how much one raw point of that type takes from each pool, with a missing entry at 1, and `health: { resist: { <type>: m } }` multiplies every pool for that type on that entity; multipliers fall in [0, 64]. Untyped damage is ×1 everywhere, so a mod without types or layers behaves as it does today, except that hide-at-full also counts an overlay's shield pair. Policies read a Bool per declared type, and type identity never enters the IR as a value (`research.md` §Commitments touched).
- **Drain order and spillover are engine rules in the damage chokepoint, for every producer (owner: every source drains, Borderlands lean).** The incoming amount, after hit zones as today, meets each layer in order: a layer above zero takes `raw × m`, and when it empties only the raw remainder it could not absorb moves on, so a shock bonus against shields never carries over to flesh. A multiplier of 0 bypasses a layer without touching it, a zero-valued layer is skipped, health takes the remainder × its multiplier, and the death latch, the sweep and `target_was_live` keep reading health only.
- **Spillover, recharge shape and no-fill-on-raise are engine taste points.** `scripting.md` §1 says the engine owns no game's taste and §11 places recharge curves in author policy; these defaults diverge because each ships a Borderlands-correct rule a novice can use untouched. Each grows along a named axis whose extensions default to today's rule: spillover and recharge shape along the authored-policy axes in Non-goals, fill on raise through `grantLayer`.
- **Every producer carries a type.** Weapons carry a weapon-level `damageType` (hitscan, projectiles, splash, NPC ranged attacks and traps), NPC melee attacks carry `attacks.<name>.damageType` (beside `weapon` it rejects load, as inline contact stats do), movers carry the FGD KVP `crush_damage_type`, and the script verb `.damage(n, { type })` takes the damage-type handle. An unknown name in FGD warns once at level load and applies untyped, because level content is not mod vocabulary.
- **Recharge is an engine per-tick host step at tick start, so every hit in tick N lands after that tick's recharge.** Any hit with a positive incoming amount, wherever it lands, restarts every one of the entity's layer delays, even a hit fully absorbed or bypassed. The last-hit stamp is live state: time spent latched counts toward the delay and hits on a latched entity still restamp. Only a hit sets the stamp; spawn, respawn, carry and grants never touch it, so a layer below capacity with no hit since arrival recharges from its first tick.
- **Recharge skips any entity at health ≤ 0, latched or not, though a lowered capacity still clamps its layers.** Respawn refills every layer to capacity, and `setHealth` and `grantHealth` leave layers alone.
- **Layer values follow health's lifecycle:** spawn fills every layer, and a fresh co-op join arrives full at its seeded capacity, while a layer not full at its pre-seed capacity when the seed lands keeps its value. Level change, same-map restart and rejoin carry each value only when health carries, clamped to the arrival capacity evaluated with the seat's per-owner values. Hot reload keeps each layer's value by name, clamps kept layers, fills added ones and drops removed ones.
- **One quantity, "points dealt", means damage everywhere downstream of the chokepoint.** Points dealt is the total drained from layers plus health after type multipliers and resistances, and `@impact.amount` is points dealt, matching `research/combat-events.md`'s post-mitigation `damage`, because the plain name serves the common reader: damage numbers, lifesteal, stagger. `@impact.raw` is the incoming amount after hit zones and before type and layers (combat-events' `rawDamage`), `@impact.absorbed` is the layers' share of points dealt, and `healthBefore` / `healthAfter` stay health-only, so an unfloored `healthAfter` still yields overkill.
- **`@impact.broke` is true when this hit took some layer from above zero to zero, and each declared type adds a Bool.** The contributor ledger, kill credit and brain threat record points dealt, so a player who only stripped a shield is a contributor. Brain damage recency resets on any hit to a live target with a positive raw amount, and a hit on a target that is not live (latched, or at health ≤ 0) credits no contributor and leaves brain memory as the lethal hit froze it, while its impact facts stay truthful, so rewards gate in policy.
- **A layer handle's `.current` / `.capacity` is a per-player read over the player's owner-private layer slots:** a plain read means the local player wherever a plain per-player read does (HUD `Bar`, store, crossings, plain impact-policy reads), and the evaluated player in a player-event condition. It takes `byPlayer` with `impact.source` and `on.player` only, and a layer read inside an interaction-pair expression rejects load, so pair-scope layer reads stay a non-goal.
- **Reads of a specific entity go through per-scope accessors that follow each scope's `state(name)`:** `impact.target.defense(h)` in an impact policy, the free function `defense(h)` in a brain guard (the brain's own entity), and `entity.defense(h)` in the overlay's `damagedEnemies` callbacks, each yielding `.current` / `.capacity` in that scope's value form. The overlay's "hide at full" counts its own shield pair: an entity is full only when health and that pair are both full.
- **The player's layer slots are owner-private:** `player.defense.<name>` and `player.defense.<name>.capacity`, keyed by declared layer name, read absent for a pawn without that layer. Declaring a layer changes the state-schema fingerprint, not `WIRE_VERSION`, as the owner-private weapon-sample slots did (`networking.md` §Version gates).
- **Non-goals:**
  - Status effects and damage over time: roadmap `status effects / DoT` owns them.
  - Crits: roadmap `crit math` owns them.
  - Shield special effects (nova, spike, absorb-to-ammo): roadmap `shields + shield types`, narrowed to special effects, owns them.
  - Authored recharge and overflow (per-layer `overflow`, recharge-scope inputs, a per-layer recharge expression): roadmap `authored defense-layer recharge and overflow policy`, added at promotion, owns them.
  - Per-action damage types. Weapon-level only; augments may add one later (`research/weapon-model.md` §3).
  - Teammate shield bars: a roadmap item added at promotion owns them. Owner-private values never leave their owner; a teammate HUD is its own replication decision.
  - Client prediction of layer values. Host samples are enough for a bar.
  - Impact policies for script and environmental damage. Roadmap "DoT / environmental death policy" owns that; layers still drain on those paths.
  - Pair-scope layer reads: the roadmap item "Per-entity state reads in interaction-pair expressions" owns them, reworded toward layer reads.

### Scripting surface
```ts
// scripts/defenses.ts — mod-wide vocabulary in its own module, as the dev mod's faction names live in scripts/ modules
import { defineDamageType, defineDefenseLayer } from "postretro";
export const shield = defineDefenseLayer("shield");
export const armor = defineDefenseLayer("armor");
export const shock = defineDamageType("shock", { against: { shield: 2, armor: 0.5, health: 0.75 } });
export const corrosive = defineDamageType("corrosive", { against: { shield: 0.5, armor: 2 } });

// every other file below imports the handles from there, never from start-script.ts, which imports them all
import { armor, corrosive, shield, shock } from "./defenses"; // "./scripts/defenses" from start-script.ts
import { defense, defineEntity, defineImpactEvent, defineMod, defineReaction,
  getMapEntities, npcs, read, when, type PlayerEventParams, type TriggerEventParams } from "postretro";
import { Bar, damagedEnemies } from "postretro/ui";
import { progression } from "./combat-lifecycle"; // gains per-owner `shieldCapacity` and `shieldTech`, shaped like `xp`
import { leveling } from "./leveling";
// Each new persisted slot needs a minted identity entry, as `progression.xp` has: `postretro-tool mint-identity dev`.

// player.ts components — capacity and recharge read per-owner progression
health: { max: 100, layers: [{
  layer: "shield", capacity: read(progression.shieldCapacity).plus(50),
  rechargeDelayMs: 2500, rechargeRate: read(progression.shieldTech).times(4).plus(20),
}] },

// reference-enemy.ts components — armored, shock-resistant, a corrosive claw, retreats once its armor breaks
health: { max: 70, layers: [{ layer: "armor", capacity: 120 }], resist: { shock: 0.5 } },
behavior: { /* … */
  attacks: { claw: { damage: 15, damageType: "corrosive", maxRange: 2, cooldownMs: 800 } },
  transitions: { engage: [{ to: "retreat", when: defense(armor).current.le(0) }] }, // own layer, as `state(name)` reads own state
},

// a weapon-level type, and typed script damage
const arcRifle = defineEntity({ canonicalName: "arc_rifle", components: { weapon: { damage: 12, damageType: "shock", /* … */ } } });
const overload = defineReaction("turret.overload", npcs({ tag: "turret" }).damage(40, { type: shock }));

// player-events.ts — the existing level-up also raises capacity, then fills health and shield to the new capacity
const levelUp = defineReaction("leveling.levelUp", (on: PlayerEventParams) => [
  on.player.addSlot(leveling.level, 1), /* …its existing steps */
  on.player.addSlot(progression.shieldCapacity, 25), on.player.grantHealth(100), on.player.grantLayer(shield),
]);

// combat-demo-reaction.ts setupLevel — a booster pad adds 25, bound as the ammo pickup is
const booster = defineReaction("combat.shieldBooster", (on: TriggerEventParams) => on.activators.grantLayer(shield, 25));
reactions.push(booster);
triggerEvents.push(...getMapEntities("trigger", { tag: "shield_booster" }).map((pad) => pad.on("enter", [booster])));

// hud.ts — the local player's shield bar
const shieldBar = Bar({ bind: shield.current, max: shield.capacity, fill: [0.3, 0.6, 1, 1], background: [0.05, 0.07, 0.1, 0.9] });

// combat-presentation.ts — damagedEnemyOverlay's source gains a shield pair over each enemy's layer
over: damagedEnemies({ lingerMs: 2_500, hideAtFull: true,
  shield: { value: (entity) => entity.defense(shield).current, max: (entity) => entity.defense(shield).capacity } }),

// combat-lifecycle.ts — refund a share of a shock shield-break on a live target to the attacker; mark a target whose armor is gone
export const shieldBreak = defineImpactEvent("shield-break", { tag: "enemy" }, (impact) => [
  when(impact.broke.and(impact.type(shock)).and(impact.target.healthBefore.gt(0)),
    [impact.source.grantLayer(shield, impact.absorbed.times(0.25))]),
  when(impact.target.defense(armor).current.le(0), [impact.target.setState("exposed", 1)]),
]);

// start-script.ts — registration
defineMod({ defenseLayers: [shield, armor], damageTypes: [shock, corrosive], events: [/* … */ shieldBreak], /* … */ });
```
Luau mirrors every name, with each accessor following its `state(name)` twin: `Postretro.defineDefenseLayer("shield")`, `shield.current`, `defense(armor).current:le(0)`, `impact.target:defense(armor)`, `entity:defense(shield)`, `impact.type(shock)`, and so on. Both runtimes ship.

## Acceptance

### Declaration and validation
- [ ] A mod that declares layer kinds and damage types loads, in TS and in Luau, producing identical descriptors.
- [ ] Load fails, naming the key path, for each of: an undeclared layer in `layers` or `against`; an undeclared type in `resist`, a weapon or an attack; `damageType` on an attack that names a `weapon`; a duplicate declared name; a layer named `health`; a layer kind listed twice in one descriptor's `layers`; an out-of-range literal; an unknown key inside `health`. A valid sibling of each loads, including one descriptor listing two different layer kinds.
- [ ] An undeclared layer or type read in an impact policy, a brain guard, an overlay callback or a layer parameter fails load naming the key path. A grantLayer or `.damage` reaction naming one is dropped at install with a log naming the reaction, and its siblings install.
- [ ] A layer expression with a per-owner read loads on the player entity and fails load on an enemy descriptor.
- [ ] An unknown FGD `crush_damage_type` warns once at level load and crushes untyped.

### Drain
- [ ] A mod with no types or layers: health results and every existing impact fact are identical to before, so the existing chokepoint, contributor-ledger, impact-policy and overlay tests pass unmodified, except where hide-at-full now also counts an overlay's shield pair.
- [ ] Ordered spillover: shield 30, armor 50, health 100; an untyped 100 leaves 0 / 0 / 80.
- [ ] Spillover in raw units: shock (shield ×2, health ×0.75) for 20 against shield 30 empties the shield on 15 raw, and health loses 3.75.
- [ ] Multiplier 0 on a layer leaves it untouched and passes the hit on. Multiplier 0 on health deals no health damage. `resist` scales on top, and `resist` 0 makes the entity immune to that type. An immune hit still restarts the delay and credits no contributor (R-P11).
- [ ] A fully absorbed hit leaves health unchanged, dispatches the impact with `absorbed` and `amount` both equal to the points drained, and never latches death.
- [ ] For a shock 20 hit on shield 30 with health ×0.75: `raw` is 20, `amount` is 33.75 (30 shield + 3.75 health), `absorbed` is 30, and `healthBefore − healthAfter` is 3.75. For a hit bypassing every layer, `absorbed` is 0 and `amount` equals the health lost.
- [ ] A zero-valued layer is skipped and never reports `broke`.
- [ ] Two hits on one tick drain in order: the second sees the first's result, and only the hit that empties a layer reports `broke`.
- [ ] Each producer drains a test shield by its type's multiplier: hitscan, direct projectile, splash, NPC melee, NPC ranged, trap, crush and the script verb, and a host-validated client hit, typed from the host's weapon descriptor.
- [ ] Producers on one tick drain in stage order, and a flushed client hit drains after them: crush, then hitscan, then a flushed client hit, 20 each against shield 30, leave health 70, and only the hitscan reports broke (R-P1).
- [ ] A capacity-0 layer reads 0 / 0, is skipped by every hit with no broke, takes no grant while its capacity evaluates to 0, and counts as full for hide-at-full (R-P9).
- [ ] A lethal hit counts overkill: shield 10, health 5, untyped 30 gives amount 30, absorbed 10 and healthAfter −15, and the ledger records 30 (R-P10).
- [ ] A hit on an entity that is not live (latched, or at health ≤ 0) drains its layers and restarts the delay, but credits no contributor, leaves brain memory as the lethal hit froze it and fires no second death report (R-P12). When it empties a layer, the impact still reports broke and a broke-gated policy runs (R-P22).

### Recharge
- [ ] Nothing recharges before the delay. After it, the layer ramps at the rate and stops at capacity.
- [ ] A hit mid-ramp restarts the delay; so does a fully absorbed or bypassed hit. A rate-0 layer never recharges. A delay of 0 starts recharging on the next tick.
- [ ] An entry that omits rechargeDelayMs first recharges 3000 ms after a hit, and one that omits rechargeRate never recharges.
- [ ] A delay lowered mid-delay, by a store write or a hot reload, applies from the next tick against the original hit: a hit on tick N, then a delay cut from 3000 to 500 ms on tick N+60, first recharges on tick N+61 (R-P20).
- [ ] A hit on the tick the delay expires: that tick's recharge applies first, the hit then drains, and the delay restarts.
- [ ] An expression parameter tracks a store change on the next tick. Lowering capacity clamps the value; raising it does not fill. A faulted expression keeps each entity's own last good value and warns once per descriptor and field: two faulting entities of one descriptor warn once, and a second faulting field warns again. One that faults before any good value reads its field's omitted default, or 0 for capacity, and a hot reload re-arms the warning.
- [ ] A capacity whose expression faults at spawn spawns the layer at 0 / 0 and warns once; once it evaluates to 80 that evaluation reads 0 / 80, and at rate 60 points/s the tick ends at 1 / 80. It fills only by recharge, which starts that tick with no hit to wait on, or a grant (R-P17, R-P19).
- [ ] A capacity reading the entity's own @state.tier tracks an impact policy's setState on the next tick. A layer parameter reading an engine per-player slot, a layer's own current or capacity included, fails load on every descriptor, the player's included. So does a `defense(h)` read of any layer, the entity's own included.
- [ ] An entity latched at zero health does not recharge, and neither does one at zero health that is not yet latched, such as a pawn killed by frame-end script damage. Respawn refills every layer. Its capacity still tracks the store, and a lowered capacity clamps it (R-P23).
- [ ] A pawn revived after lying latched longer than the delay starts recharging on the first tick that begins with it above zero health, while a hit taken while latched restarts the delay from that hit (R-P14). A carried layer below capacity starts recharging on the first tick after arrival (R-P13).
- [ ] A hit after the tick restarts the delay exactly as a hit inside the tick does: a flushed client hit and frame-end script damage each first recharge on the same tick as a weapon hit, in co-op and in single player (R-P2).
- [ ] Only a hit sets the last-hit stamp: a pawn killed on tick N and respawned at 100 / 100 before its delay expires, whose capacity then rises to 150, reads 100 / 150 and first recharges 180 ticks after the lethal hit, by rate × dt (R-P27).
- [ ] Delay expiry is tick-exact: a 3000 ms delay at 60 Hz first recharges 180 ticks after the hit, by rate × dt, and does the same after long uptime (R-P3).
- [ ] A respawn fills to the capacity in force that tick, even when the store raised it the tick before. A capacity lowered mid-tick by a store write alone, with no grant after it, applies from the next tick, so a hit later in the same tick drains against the old capacity (R-P4).
- [ ] Expression results clamp into the literal ranges: a capacity of 2e6 evaluates as 1e6, −5 as 0, and a delay of 90000 ms as 60000. A non-finite result is a fault and keeps the last good value.
- [ ] A connected client runs no layer recharge and evaluates no layer parameter. Its layer reads come only from the host. A parameter that faults warns on the host and never on a connected client.

### Grants
- [ ] `grantLayer(layer)` fills to capacity, and `grantLayer(layer, n)` adds n, clamped to capacity. Neither resets the recharge delay: a grant mid-delay leaves recharge starting at the original time.
- [ ] A level-up reaction that raises capacity and then grants health and layers in one fire restores both to full on the same tick, at the new capacity. For the rest of that tick, `.capacity` reads the new capacity and never below `.current`, and frame-end script damage after it drains from the new full value.
- [ ] A crossing-fired reaction that raises the capacity slot and then calls grantLayer(shield) fills to the new capacity when it dispatches, as the same body does on a player event.
- [ ] A crossing-fired body that grants shield 25 and then damages 20, against shield 0 / 100 and health 100, leaves shield 5 / 100 and health 100; the reverse order leaves shield 25 / 100 and health 80 (R-P26).
- [ ] On a reaction fired by address, a store write to a pawn whose seat was released earlier that frame warns and is skipped at dispatch, and the store holds no value for that seat.
- [ ] An impact policy that raises impact.source's capacity slot from 100 to 150 and then calls impact.source.grantLayer(shield) leaves the attacker at 150 / 150 on that dispatch, while the grant's amount and the policy's gates still read the snapshot from before the write.
- [ ] A grant clamps to the capacity it evaluates, then adds: at 80 / 100, a store write lowering capacity to 40 and then grantLayer(shield) leave 40 / 40, and a 30-point hit that tick leaves 10 / 40. On a full 100 / 100, raising capacity to 125 and then grantLayer(shield, 25) reads 125 / 125 (R-P18). When the expression faults on the next tick, the layer keeps the grant's 40 (R-P21).
- [ ] A negative amount warns and does nothing. A recipient without that layer warns and is skipped, while its siblings are still granted. `impact.source.grantLayer` credits the damager on any hit that runs impact policies, including a host-validated client hit flushed after the tick.
- [ ] impact.source.grantLayer(shield, impact.absorbed.times(0.25)) after a hit that absorbs 40 grants the attacker 10. A grantLayer reaction with a non-finite or non-numeric amount rejects the setup manifest, as a bad grantHealth does, in both runtimes.
- [ ] A grant and hits on one tick apply in tick order, frame-end script damage included: a hit after a grant drains the granted points and reports broke on its impact dispatch when it empties them, though frame-end script damage runs no policy. grantLayer on a latched entity fills the layer and leaves the latch set (R-P7).
- [ ] A call-site gate holds layer value writes to the damage chokepoint, recharge and its capacity clamp, grantLayer, spawn and its join-seed fill, hot reload, respawn and carry restore.

### Lifecycle
- [ ] Level change, same-map restart and rejoin carry a player's layer values, clamped to the new capacity. Spawn fills. Hot reload clamps kept layers, fills added ones, an expression capacity included (R-P15), and drops removed ones.
- [ ] An entity spawned mid-tick with an expression capacity is full at once: a hit on its spawn tick drains the layer, not health (R-P5).
- [ ] A hot reload during the delay keeps each layer's value by name across a reorder, and keeps the last-hit stamp, so an unchanged delay starts recharge on the same tick. A capacity edited into an expression that faults keeps the entity's last good capacity and warns once (R-P8).
- [ ] setHealth and grantHealth leave a 10 / 100 shield at 10, including on a latched pawn they revive. Respawn fills it to 100, and a carry restore sets its carried value.
- [ ] A carried layer keeps its value when the capacity reads a per-owner slot: 120 / 150 restores as 120 / 150, and the first tick after arrival recharges from there. A seat that carried nonpositive health arrives with every layer full (R-P6).
- [ ] A fresh co-op join whose seeded per-owner values raise its capacity arrives full at that capacity: a returning player whose saved progression makes the shield 150 joins at 150 / 150, not 50 / 150.
- [ ] A join seed that lands after the pawn materializes still brings an unhit pawn to full at the seeded capacity on the first tick after it lands, one that materialized at 0 / 0 included (R-P28), while a pawn hit to 30 / 50 before it lands reads 30 / 150 (R-P16). A pawn hit and then refilled to 50 / 50 before it lands counts as full and reads 150 / 150 (R-P25), and the refill runs before the first hit of the tick after it lands (R-P24).

### Reads
- [ ] An impact policy reads `absorbed`, `broke`, the per-type Bool, and the target's layer current and capacity after the hit through the target accessor. A plain `shield.current` in the same policy reads the local player's layer.
- [ ] An enemy brain guard reads its own layer and changes state when the layer breaks.
- [ ] The overlay's shield pair reads the overlay entity's layer. A shield-only hit at full health keeps the overlay shown, on the host and on a connected client. An entity at full health with a full shield pair hides, even while an undisplayed armor layer is below capacity.
- [ ] The player's layer current and capacity read the host value on the host and on a connected client. A pawn without that layer reads absent, on the host and on that pawn's client, never the host pawn's value.
- [ ] With two connected clients, each receives only its own pawn's layer slots. The other client's layer values never appear in its snapshots.
- [ ] In a player-event condition, a plain layer read means the player being evaluated. shield.current.byPlayer(impact.source) in an impact policy reads the attacker's layer, and shield.current.byPlayer(on.player) in a player-event reaction reads that player's layer. A layer read in an interaction-pair expression, by an assist side or plain, fails load.
- [ ] A player who only drained an enemy's shield is credited as a contributor when the enemy dies. Brain threat counts points dealt. A fully absorbed hit resets brain damage recency.
- [ ] The Scripting surface example runs as `content/dev` content and loads clean in both runtimes.
- [ ] `docs/scripting-reference.md` documents the vocabulary, the drain rules and a Borderlands recipe. Every command in it runs from an SDK bundle.
- [ ] Declaring a layer adds its current and capacity slots and changes the state-schema fingerprint, and WIRE_VERSION is unchanged.
- [ ] An overlay bound to @state.shield reads its value as it does today, and hide-at-full counts it: at full health with @state.shield below its max, the overlay stays shown. No hit or recharge ever changes @state.shield.

### Manual
- [ ] Playtest the triangle in the dev mod: shock strips a shielded enemy, corrosive eats an armored one, each is weak on the other.
- [ ] Player shield feel: delayed recharge, and a progression bump visibly speeds the recharge.
- [ ] HUD shield bar and enemy overlay shield read correctly. A co-op client's HUD bar tracks the host at the lag profile.

## Path
- **Seams:** `apply_damage_with_context` and `DamageContext` (health.rs); `DamagePayload` (foundation_pods.rs); `HealthDescriptor` and its validation (combat.rs); faction drains and resolution (`drain_factions_js`, `resolve_entity_faction_indices`) as the vocabulary precedent; `NumberOrIr` and `AssistPairTable::bind` as the literal-or-expression precedent; `PlayerConditionScope` as the "plain read means this player" precedent for the parameter scope, minus its engine slots; `IMPACT_DISPATCH_INPUTS` and `EntityScope`; `BrainInputRef::State` and `OverlayStateScope` for the `@defense.` family, with `TargetHandle.state`, brain `state` and `OverlayEntity.state` (TS and Luau) as the accessor spellings (the brain `defense` exports from `"postretro"` beside `state` and builds `.current` / `.capacity` through `runtime.read`, so `.le` returns a guard node; its Luau global joins the prelude's brain globals beside `state`); `classify_pair_read` for the pair-scope rejection; `PlayerSlot`, `engine_state_catalog`, `owner_private_source_value` and `MIRRORED_GAME_STATE_LEAVES` for the owner-private family; `CarriedState` and `CarriedField` (the destructure has no wildcard); `respawn::drain` and `begin_tick` for recharge placement.
- **Evaluation and recharge placement.** Parameters evaluate each tick at tick start, then clamp, then recharge runs (R-P19), for every entity, with only the recharge step skipping health ≤ 0 (R-P23); hits in a tick use the capacity in force (R-P4). Spawn (R-P5, R-P17), respawn (R-P4, R-P27), hot reload (R-P15) and every grant (R-P18, R-P21) evaluate capacity on demand instead; an impact-policy grant evaluates when its planned effect applies, not against the plan snapshot (`evaluate_pending_in_tick_impacts`). Recharge runs after the respawn drain. The chokepoint stamps the host tick of the last hit, which covers hits that land outside the tick (R-P2, R-P3, `research.md` §Damage chokepoint), and expiry compares ticks since the stamp with the delay in force; store the stamp, never a deadline (R-P20).
- **Body order.** On player and trigger events, `grantLayer` binds as a consequential in-tick command beside `grantHealth` and `addSlot`. On reactions fired by address, the host's `dispatch_add_owner_slot` writes the slot instead of pushing `SystemReactionCommand::AddOwnerSlot`, and a `setState` step writes instead of queuing `SystemReactionCommand::SetState`; the drain's seat-liveness, readonly and range checks (`dispatch_system_commands`, `main.rs`) move to dispatch, so dispatch needs a seat-liveness view and the session's `SetState` IR bindings in reach. The `owner_slot_writes_enabled` client gate is unchanged. On that path `grantLayer` then applies at dispatch beside `grantHealth`. `fire` targets still run after the firing body, so pin S27 (`plans/done/reaction-body-composition/research.md`) stays green; crossing evaluation and client persistence run after dispatch and see every write (`research.md` §Body order).
- **Join seed.** A seed can land after the pawn participates (`join_seed.rs`), so redo the spawn fill when it lands only for a layer still full at its pre-seed capacity (R-P16, R-P25), before the first hit of the next tick (R-P24).
- **Owner-private family.** A new name-keyed slot family, read through the one pawn-to-value lookup (`research.md` §Co-op).
- **Chosen shape vs the strongest rival.** Chosen: an engine layer stack with an authored matrix and expression parameters. Strongest rival: the same engine storage, drain and last-hit stamp, but recharge (and optionally overflow) authored as an IR expression over a layer-local scope (current, capacity, time since hit, tick delta). That is `scripting.md` §11's split read literally, and the hot IR write path `plans/done/M14--movement-dash-runtime-values` earmarked shield recharge for. The chosen shape grows into it along the recharge-shape axis in Non-goals without breaking a mod. Not now, because it builds the per-entity IR write path before any mod needs more than delay-then-rate. A weaker rival, author `@state` layers, fails for the reasons in `research.md` §Layer home.
- **First slice:** one player shield end to end, covering descriptor, drain, literal recharge and the owner-private read on a connected client. That slice falsifies the riskiest pieces: the new owner-private family and the tick-start recharge order.
- **Split first.** These files run past ~800 lines: `impact_policy.rs`, `state_slots.rs`, `scopes.rs`, `combat.rs`, `health.rs`. Split each first where the brief extends it: behavior-preserving, in its own commit.
- **Coordinate with the drafts.** On landing, revise `E10--enemy-stagger` (accumulate points dealt) and `coop--predicted-combat-feedback` to the points-dealt vocabulary (`research.md` §Amount consumers).

## Open questions
- Whether layers get a per-tick cost bound (count of layered entities × expression size) beyond the IR's own depth and node caps — **delegated**: measure on the dev mod and report in the plan of record.
- Whether a connected client's own local `setState` also moves to dispatch, or stays on the frame-end drain — **delegated**: the Decision pins only the host; choose what keeps client body order and the owner-slot write gate simple, and report it in the plan of record.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Layer declaration | executor | `defenseLayers` | `defineDefenseLayer(name)` | `Postretro.defineDefenseLayer(name)` | n/a |
| Damage type declaration | executor | `damageTypes`, `against` | `defineDamageType(name, { against })` | `Postretro.defineDamageType(name, { against = … })` | n/a |
| Descriptor layer entry | executor | `layers[].layer`, `capacity`, `rechargeDelayMs`, `rechargeRate` | same | same | n/a |
| Resistances | executor | `health.resist` | same | same | n/a |
| Weapon / attack type | executor | `damageType` in `components.weapon` and in an inline `behavior.attacks.<name>` | same | same | n/a |
| Crush type | executor | n/a | n/a | n/a | `crush_damage_type` |
| Script damage type | `ApplyDamageArgs` field | `{ amount, type }` (declared name) | `.damage(n, { type: handle })` | `:damage(n, { type = handle })` | n/a |
| Player layer read (owner-private) | catalog rows per declared layer | slot names `player.defense.<name>`, `player.defense.<name>.capacity` | `handle.current`, `handle.capacity` (`PlayerComputedRef`): `Bar` `bind` / `max`, `read()`, crossings, `.byPlayer(impact.source | on.player)` | `handle.current`, `handle.capacity`, `:byPlayer(…)` | n/a |
| Entity layer read (per-scope accessor) | IR inputs | `@defense.<name>`, `@defense.<name>.capacity` | `impact.target.defense(handle)`, brain free function `defense(handle)`, overlay `entity.defense(handle)`, each with `.current` / `.capacity` | `impact.target:defense(handle)`, global `defense(handle)`, `entity:defense(handle)` | n/a |
| Impact facts | IR inputs | `@impact.raw`, `@impact.absorbed`, `@impact.broke`, `@impact.type.<name>` (`@impact.amount` keeps its name, now points dealt) | `impact.raw`, `impact.absorbed`, `impact.broke`, `impact.type(handle)` | same, as plain fields: `impact.type(handle)` | n/a |
| Layer grant | grant chokepoint | `grantLayer` `{ layer, amount? }` (literal on group and subject tokens, Number expression on `@impact.source`) | `.grantLayer(handle, amount?)` on `players()`, the subject tokens (`on.activators`, `on.player`, …) and `impact.source` | `:grantLayer(handle, amount?)` | n/a |
