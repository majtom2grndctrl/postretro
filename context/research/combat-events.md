# Combat Events — Design Exploration

**Date investigated:** 2026-06-19
**Status:** Pre-spec exploration. Not yet a draft plan. Captures the long-term
target so the first limited-scope sprints don't paint the engine into a corner.

> **Read this when:** scoping any on-hit / on-kill behavior — XP, scoring, kill
> credit, ammo/health economy, ability meters, combo systems, damage numbers,
> on-hit procs, or player-death consequences. Also when extending the
> behavior-IR with a new binding scope.
> **Key invariant:** the engine emits structured combat *facts*; the mod owns
> the *policy*. A kill grants nothing by itself — what a kill means is authored,
> never baked. This is "scripts declare, Rust executes" (`scripting.md` §1, §11)
> applied to combat.
> **Related:** `context/lib/scripting.md` (§10 reactions, §11 typed command
> buffer) · `context/lib/entity_model.md` (§3 lifecycle/death, §7 health) ·
> `context/research/weapon-model.md` · `context/plans/done/M10--entity-health-damage/`
> · `context/plans/done/M14--behavior-ir-substrate/` ·
> `context/plans/ready/M10--enemy-ai-behavior/`

---

## 1. Vision

A contemporary FPS feel loop runs on combat *feedback*: a hit returns damage
numbers and on-hit procs; a kill grants XP, charges an ability meter, drops
ammo, or feeds a combo multiplier; movement state and weapon choice change what
all of that is worth. The aesthetic is retro; the game design is not. The
yardsticks are **Doom Eternal** (resource economy — glory kills heal, chainsaw
kills drop ammo, fodder/heavy/super-heavy classes drive the loop), **Titanfall**
and **Turbo Overkill** (mobility-rewarded combat — sliding, airborne, wall-run
kills count for more; kills charge a meter), and **Borderlands** (crits,
elemental damage, kill skills, per-weapon proficiency).

We do not ship any of those *systems*. We ship the **substrate** that lets a mod
build any of them — and lets a different mod build something we never imagined.
The engine's job is to surface, on every damage application and every death, a
rich and typed set of combat facts. The mod's job is to decide what those facts
mean. We are an opinionated, FPS-skewed engine, so the *menu of facts we
pre-compute* (hit zone, weapon, movement state, crit, attribution, enemy tier)
is itself our opinion about what shooter combat needs. A generalist engine makes
the modder derive "was this a sliding airborne crit"; we hand it over as a
first-class field.

The expressiveness ceiling is deliberate. Behavior crosses the FFI as typed,
serializable IR, never a live callback (`scripting.md` §11). The mod's combat
policy is a command buffer the engine evaluates — pure, total, bounded. That is
the only way "modders own the logic" survives the no-live-VM rule, and it is the
durable form, not a compromise.

---

## 2. Core Principles

| Principle | Invariant |
|-----------|-----------|
| **Facts, not policy.** | The engine emits combat facts. Reward, credit, scoring, and consequences are authored. The engine has no concept of "XP," "score," or "worth." |
| **One event, three moments.** | A single `combat` event schema, surfaced at three moments: per-impact, per-attack (aggregated), and per-kill. Authors subscribe to the moment they need. |
| **Target-neutral.** | A combat event is *attacker → target*. The player is a valid target. `playerDied` is the degenerate `onKill` where the target is the player — not a special case. |
| **Attribution is modder-owned.** | Each damage application carries a modder-controlled **source id**. The engine keeps a bounded per-target ledger keyed by it. The mod's policy picks credit from pre-reduced facts. Granularity (per-weapon, per-element, per-player) is whatever the mod stamps. |
| **Declare, don't drive.** | Handlers, filters, and reward math are data declared at load. The VM drops. Rust evaluates the policy each time the event fires. No live script in the combat loop. |
| **Compute over the scope, write the store.** | A computed policy is behavior-IR bound to a `CombatScope` (the event's facts) reading and writing the mod state store. Reuses the shipped IR substrate; the combat event is a new binding scope, not a new evaluator. |
| **Resources are engine-owned; currencies are mod-owned.** | Mod currencies (XP, score, meters, combo) are mod store slots, freely written. Core resources (health, ammo, armor) are engine-owned and readonly to scripts — granting them flows through a blessed chokepoint, the inverse of `applyDamage`. |
| **Kill is sweep-authoritative.** | The kill moment fires once, from the death sweep, for every confirmed death including hit-less ones (DoT, environmental, deferred-despawn). It is never derived from a co-located hit. |

---

## 3. The Combat Event

One schema. Three moments. The moments differ in *when* they fire and *which*
fields are meaningful, not in their type.

| Moment | Verb | Fires | Frequency | Field shape |
|--------|------|-------|-----------|-------------|
| **Impact** | `onImpact` | Per damage application (one pellet, one tick of DoT) | High | Singular — one weapon, one zone, one target |
| **Attack** | `onDamage` | Per attack, after its impacts resolve | Medium | Aggregate — sums, counts, per-bucket reductions |
| **Kill** | `onKill` | Per confirmed death, once, from the sweep | Low | Reduction over the target's whole damage history |

**Why per-impact *and* aggregate.** An elemental shotgun whose fire affix
applies to two of three pellets has no single element, no single zone, no single
target across the blast. Aggregation is well-defined for additive scalars (sum
damage) but undefined for categoricals that vary per impact. So the two views
carry different field *shapes*: per-impact exposes the singular facts;
aggregate exposes reductions (`totalDamage`, `critCount`, `damageOf("fire")`).
Each is lossless for its purpose. Most authors use the aggregate; per-impact is
the power path.

**Aggregation = reduction, same as the kill ledger.** The per-attack aggregate
reduces over a *set of impacts*; the kill ledger reduces over a *target's
lifetime of impacts*. Same machinery, different scope. The pre-reduced-scalar
catalog serves both.

**Aggregation window.** An attack's aggregate is well-defined only when its
impacts resolve together. Hitscan does — every pellet lands in one fire tick —
so the aggregate is clean today. Traveling projectiles (impacts arriving over
several ticks) reopen "what is the window," and are deferred alongside
projectiles themselves.

**Field validity by moment.** A field reads its real value only on the moment
where it is defined; elsewhere it reads the IR type-zero (`0.0` / `false`), per
the evaluator's totality contract — no nulls. The attacker's movement state is
meaningful on an impact, noise on a DoT kill. The attribution ledger is complete
at the kill, partial on a single impact.

---

## 4. Field Catalog

Author-facing names under the `combat` namespace. Numeric (`n`) and boolean
(`b`) fields are IR-readable leaves. **Categorical** (`cat`) fields carry no
scalar value in a number/bool IR — they are read only through an equality
predicate (`is(combat.weapon, "shotgun")`), resolved to a boolean leaf at bind.

**Tiering:** *Now* — derivable at the damage chokepoint and the death sweep with
the source-id ledger. *Grant* — requires the engine resource-grant chokepoint to
*act on* (the field itself is readable; writing health/ammo is the gated part).
*Damage-type* — gated on the future Shields + damage-type milestone
(`DamagePayload` is amount-only today).

### Per-impact (`onImpact`)

| Field | Type | Tier | Note |
|-------|------|------|------|
| `combat.damage` / `rawDamage` / `overkill` | n | Now | post-mitigation / pre-mitigation / beyond-lethal |
| `combat.killed` | b | Now | this impact dropped the target to ≤0 |
| `combat.target` / `attacker` | cat | Now | archetype or spawn-tag identity |
| `combat.targetIsPlayer` / `attackerIsPlayer` | b | Now | |
| `combat.targetTier` / `targetMaxHp` | n | Now | fodder/heavy/super ordinal |
| `combat.targetHpBefore` / `targetHpAfter` / `targetHpFractionAfter` | n | Now | |
| `combat.zone` | cat | Now | `head` / `limb` / `weakpoint` (skeletal hit zones) |
| `combat.wasCrit` | b | Now | |
| `combat.weapon` | cat | Now | |
| `combat.wasMelee` / `wasSplash` / `wasHitscan` | b | Now | |
| `combat.attackerState` | cat | Now | movement FSM state — `sliding`/`airborne`/`wallRunning`/`dashing`/… |
| `combat.attackerSpeed` / `distance` | n | Now | |
| `combat.element` | cat | Damage-type | |
| `combat.brokeShield` / `brokeArmor` | b | Damage-type | layered defenses |

### Per-attack aggregate (`onDamage`)

| Field | Type | Tier | Note |
|-------|------|------|------|
| `combat.totalDamage` | n | Now | summed over the attack's impacts |
| `combat.impactCount` / `hitCount` / `critCount` | n | Now | |
| `combat.targetCount` / `killCount` | n | Now | distinct targets / kills caused |
| `combat.damageOf(source)` | n | Now | bucketed by source id |
| `combat.damageOf(element)` | n | Damage-type | bucketed by element |
| `combat.weapon` / `attacker` / `attackerState` | cat | Now | uniform across one attack |

### Per-kill (`onKill`)

| Field | Type | Tier | Note |
|-------|------|------|------|
| `combat.target` / `targetTier` / `targetMaxHp` | cat / n | Now | the dead target's identity |
| `combat.targetIsPlayer` / `targetIsBoss` | b | Now | `playerDied` discriminator |
| `combat.totalDamage` | n | Now | total dealt to this target over its life |
| `combat.damageBy(source)` | n | Now | per-source, from the ledger |
| `combat.lastHitWeapon` / `topContributorWeapon` | cat | Now | attribution candidates |
| `combat.lastHitShare` / `topContributorShare` | n | Now | fraction of lifetime damage |
| `combat.killShotWasHeadshot` / `killShotWasCrit` | b | Now | the lethal blow's facets |
| `combat.wasMelee` / `wasGloryKill` / `wasExecution` / `wasEnvironmental` | b | Now | method |
| `combat.timeToKill` / `overkill` | n | Now | first-damage-to-death seconds |
| `combat.attackerState` | cat | Now | killer's state — type-zero for hit-less deaths |

---

## 5. Author API

End-state shapes, written to win the marathon — the complete intended surface,
not the first slice. Mirrors the existing SDK idiom: `define*` builders,
manifest-declared descriptors with optional `levels` scoping (like reactions and
crossings, `scripting.md` §2), the `runtime` IR namespace and `read(name)` leaf
(§11), and a `combat` reference tree obtained inside a handler (parallel to
`getGameState()`). TypeScript shown; Luau is the behavioral twin
(`require("postretro")`).

```ts
// Proposed design.
import {
  defineMod, defineStore, defineCombatHandler,
  getCombatEvent, is, runtime, addStore, grant, fire,
} from "postretro";
```

**Currencies are declared store slots.** Mod-owned, freely written by combat
policy. Engine resources (`player.health`, `player.ammo`) are never declared
here — they are engine-owned and granted, not set.

```ts
// Proposed design.
const { state: progression } = defineStore("progression", {
  xp:        { type: "number", default: 0, persist: true },
  styleMeter:{ type: "number", default: 0 },
  // Per-weapon proficiency — the original motivating case. One slot per class.
  xpByWeapon:{ type: "number", default: 0, perKey: true }, // xpByWeapon.shotgun, …
});
```

**The simplest handler — flat accumulate, no IR.** A `do` of `addStore` is a
read-modify-write the engine performs; a constant delta needs no command buffer.
`when` is the subscription filter (a categorical/boolean predicate over the
event) so the handler only fires for matching events.

```ts
// Proposed design.
// "Count enemies cleared in room 3" — worldQuery/map tags applied at setup, a
// counter incremented per kill.
defineCombatHandler({
  on: "kill",
  when: is(getCombatEvent().target, "room3Enemy"),
  do: addStore("progression.kills", 1),
});
```

**Computed reward — behavior-IR over the combat scope.** `do` of an IR
expression binds a `CombatScope` (the event's facts) composed with the store
(read current, write back). Headshot kills are worth double:

```ts
// Proposed design.
const c = getCombatEvent();
defineCombatHandler({
  on: "kill",
  when: is(c.target, "grunt"),
  do: addStore("progression.xp", runtime.select(c.killShotWasHeadshot, 100, 50)),
});
```

**Mobility-rewarded combat (Titanfall / Turbo Overkill).** The reward reads the
killer's movement state:

```ts
// Proposed design.
const c = getCombatEvent();
defineCombatHandler({
  on: "kill",
  do: addStore("progression.styleMeter",
    runtime.select(is(c.attackerState, "sliding"), 50, 10)),
});
```

**Per-weapon proficiency** — attribution decides the bucket. The credited
weapon is a categorical fact; the per-key slot it writes is chosen from it:

```ts
// Proposed design.
const c = getCombatEvent();
defineCombatHandler({
  on: "kill",
  do: addStore(byCredit("progression.xpByWeapon", c.lastHitWeapon), 25),
});
```

**Resource economy (Doom Eternal).** Granting engine resources uses `grant`, the
blessed chokepoint — never a store write, because health/ammo are engine-owned
and readonly to scripts:

```ts
// Proposed design.
defineCombatHandler({ on: "kill", when: c.wasGloryKill,            do: grant("player.health", 25) });
defineCombatHandler({ on: "kill", when: is(c.lastHitWeapon, "chainsaw"), do: grant("player.ammo", 20) });
```

**Damage numbers / on-hit feedback (Borderlands).** The high-frequency
per-impact moment drives presentation. (UI binding is `ui-layer.md`'s surface;
shown here only to demonstrate the event reaches it.)

```ts
// Proposed design.
defineCombatHandler({
  on: "impact",
  do: fire("showDamageNumber", { value: c.damage, crit: c.wasCrit }),
});
```

**Player death is one more kill handler.** `playerDied` is not a distinct event:

```ts
// Proposed design.
defineCombatHandler({
  on: "kill",
  when: c.targetIsPlayer,
  do: fire("playerDied"), // a named reaction the mod wires to its death-flow policy
});
```

**Attribution granularity is a weapon-descriptor choice.** Each damage producer
stamps a **source id**, defaulting to its canonical name. A mod that credits by
damage-type instead of weapon overrides it:

```ts
// Proposed design.
const flamethrower = defineWeapon({
  // …
  creditSource: "fire", // ledger keys on "fire", not "flamethrower" — per-element credit
});
```

The `do` verbs in one place:

| Verb | Effect | Path |
|------|--------|------|
| `fire(name, args?)` | Dispatch a named reaction/event | Existing reaction registry (`scripting.md` §10) |
| `addStore(slot, delta)` | Read-modify-write a mod currency. `delta` constant → no IR; `delta` a `runtime` expr → IR | Mod store slot (script-capability) |
| `grant(resource, amount)` | Add to an engine-owned resource | Blessed resource chokepoint (inverse of `applyDamage`) |

---

## 6. Attribution

"Who gets credit for the kill" is the crux of per-weapon / per-element scoring,
and it is a **last-hit attribution problem** by nature: damage accumulates across
ticks and sources, and HP reaches zero in the sweep, divorced from any single
shot. Credit cannot be read off the weapon — it must come from data recorded as
damage lands.

**The split: ledger (engine) vs. rule (mod).** The engine owns the *recording* —
a bounded per-target contributor ledger keyed by source id, accumulated at the
`apply_damage` chokepoint, capped in size by construction (the
4096-particle-cap precedent). Recording facts is a *noun*, not a policy. The mod
owns the *rule* — last-hit, most-damage, threshold, weighted — expressed as IR
over the ledger's pre-reduced fields. The dev mod ships a reference rule (e.g.
last-hit XP) as **example IR a mod replaces wholesale**.

**Bounded by the IR's deliberate Turing-incompleteness.** The IR has two scalar
value types and no iteration (`scripting.md` §11). A mod therefore cannot loop
over an arbitrary contributor set. The engine pre-reduces the bounded ledger into
scalar leaves — `totalDamage`, `damageBy(source)`, `topContributorShare`,
`lastHitWeapon` — and the mod's rule *selects and combines* them. Last-hit and
most-damage are both a `select` over pre-reduced facts; no iteration needed.

**The expressiveness fork (deferred).** Rules beyond "select among pre-reduced
facts" — "credit every source above 30% across the full contributor set" — need
either one more engine-computed parametric leaf (`contributorFractionOf(source)`,
cheap, added on demand) or a **bounded-fold IR node** (a capped reduction with a
local accumulator binder — a real substrate extension to the IR's type system).
Both stay total and bounded. Neither is built until a mod needs a reduction the
engine did not pre-bake. The pre-reduced-scalar surface is the foundation either
would extend, so nothing here forecloses it.

---

## 7. Engine Ownership

**Two emission sites, one schema.** The damage chokepoint emits impacts (and the
per-attack aggregate); the death sweep emits kills. They populate the same
schema but are produced at different frame stages
(`entity_model.md` §5: weapon fire tick → death sweep). Keeping them separate is
load-bearing — see the kill-authority principle (§2). A hit that kills carries an
informational `combat.killed` for immediate feedback (gib, damage-number color);
the authoritative kill — economy, counters, attribution — fires once from the
sweep, covering DoT, environmental, and animation-deferred deaths a co-located
hit would miss.

**The combat scope is a new `BindingScope`.** It composes the event's facts
(read-only) with the mod store (read/write, script-capability). Behavior-IR is
evaluated at a per-event bind+eval site in the combat path — distinct from the
context-free system-command drain, because the policy needs *this event's*
context. The substrate, evaluator, versioning, and store read/write are already
shipped (`plans/done/M14--behavior-ir-substrate/`); this adds a scope and an
evaluation site, not a new evaluator.

**The resource-grant chokepoint** is the one genuinely new engine mechanism
beyond recording and emission: a validated, engine-owned path that *adds* to
health/ammo/armor, mirroring `applyDamage` in reverse. It exists because the
events let a mod compute "how much," but engine resources must not be raw script
writes.

**Coordination.** Recording and the kill moment touch the same death-sweep seam
the in-flight enemy-AI plan reworks for animation-deferred despawn
(`plans/ready/M10--enemy-ai-behavior/`). Build on that seam, do not race it.

---

## 8. Non-Goals (this exploration)

- **Bounded-fold IR node / generic reductions over the contributor set** — the
  expressiveness fork (§6). Demand-gated; pre-reduced scalars ship first.
- **Elemental and layered-defense fields** (`element`, `brokeShield`,
  `brokeArmor`, `damageOf(element)`) — gated on the Shields + damage-type
  milestone; `DamagePayload` is amount-only today.
- **Projectile aggregation window** (§3) — deferred with projectiles.
- **Multi-kill / streak / combo *windows*** — time-windowed aggregation is mod
  policy over store slots plus a decay tick, not an engine combat fact.
- **A live script callback on hit or kill** — forbidden by the no-live-VM rule.
  Policy is IR, always.

## Delivery shape

An epic of roughly five sequential specs, front-loaded on the source-id +
ledger recording contract (the hard-to-reverse data shape), then a kill-first
thin slice, the hit moments, the `CombatScope` IR adopter, and a dev-mod
integration. Detailed decomposition belongs in `/draft-plan`, not here.
