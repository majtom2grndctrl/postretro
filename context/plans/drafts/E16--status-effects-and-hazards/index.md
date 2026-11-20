# E16--status-effects-and-hazards

Brief · resumable · Epic 16 · reads: `context/lib/scripting.md` §2, §10.8, §11, §12 · `context/lib/entity_model.md` §Components, §5 Update Order, §7 Environment Volumes · `context/lib/networking.md` §Combat authority, §What gates, and what replicates instead, §Version gates · `context/lib/testing_guide.md` §Resource bounds · read at e7438238 · evidence: `research.md`

Builds on `plans/done/E16--damage-types-and-defense-layers` (typed damage, layers, the reserved status doors) and `plans/done/E16--impact-policy-substrate` (the producer gate this brief retires).

## Problem
The owner wants burn, shock and corrode effects, and lava and slime that hurt, all table stakes for the genre. None can be authored today, and damage that does not come from a weapon or AI hit skips authored consequences:
- **No status mechanism exists.** The engine has no per-entity effect that deals damage over time, so DoT can only be faked with script `wait` loops.
- **Fluids cannot hurt.** A fluid declaration has movement and presentation fields only; `plans/done/E24--water-and-environment-volumes` left fluid damage as "a later field" on the fluid's own data.
- **Script damage runs no impact policy.** The impact evaluator skips every impact the `applyDamage` reaction produces, by design of the v1 producer gate. An enemy killed by a scripted hurt volume stays a zero-health corpse: no authored death animation, no despawn, no kill report.

When this is done, a mod declares statuses, applies them from impact policies, reactions and fluids, and every status tick drains layers and health through the damage chokepoint with a type and a source, runs impact policies, and credits whoever applied it. Players see a status on themselves through the HUD and on others through attached particles and sound, in co-op as well. Every impact runs its policies, whatever produced it.

## Decisions
- **A status is an engine-owned noun with author-named kinds,** following defense layers (`plans/done/E16--damage-types-and-defense-layers`, `scripting.md` §11: the engine ships the component and its per-tick system, the author ships the policy). `defineStatus` joins the binding-name sugar; `defineMod({ statuses: [...] })` registers kinds, a handle crosses as `{ kind: "status", name }`, and a bad reference fails where a bad damage-type reference fails on that surface. Load fails past 64 statuses in one mod.
- **Parameters are literals:** `damage` per tick per stack [0, 1e6], `type` (a damage-type handle; omitted takes the mod's default type), `tickMs` [one fixed tick, 60000], `durationMs` [`tickMs`, 600000] or omitted for "until cleared", `stacking` and `maxStacks` [1, 64, default 1]. Expressions are a later axis; resistance already varies through the type matrix, because ticks are typed hits.
- **An instance is keyed by status and applier.** The applier is the applying impact's damager; with no damager it is the applying source id (`fluid.<name>`, `script.applyStatus`). Separate appliers keep separate instances, so each tick credits the right player in co-op. An entity holds at most 16 instances; a new one when full replaces the instance with the least time left (`research.md` §Instance cap).
- **Stacking is per instance:** `"refresh"` (default) resets remaining time; `"stack"` adds one stack up to `maxStacks` and resets remaining time, and a tick deals `damage × stacks`; `"ignore"` leaves a live instance untouched. All applications by one applier to one target on one fixed tick count as one, so eight shotgun pellets or a rocket's hit plus its splash apply once. This is the per-attack question `research/combat-events.md` §3 raised, settled without an aggregate moment.
- **Tick timing:** an instance first deals damage `tickMs` after it is created, then every `tickMs`, on fixed-tick boundaries. Refresh and stack never move the tick phase, so a fluid refreshing every tick still ticks. A tick due on the tick the instance's time runs out still lands, then the instance ends.
- **Ticks are host-authoritative in-tick hits:** a status stage runs after Weapons and before the death sweep (`entity_model.md` §5), so a lethal tick latches and credits on its own tick. Each tick enters the damage chokepoint with the status's type, the applier as damager and the applying hit's source id (`fluid.<name>` or `script.applyStatus` without one), so it drains layers, restarts layer delays, records the contributor, feeds brain damage memory and runs impact policies with `impact.source` as the applier. Clients predict no status and no tick (`networking.md` §Combat authority).
- **Statuses clear when the death latch sets, on respawn, and on level change, same-map restart, rejoin and save:** they are transient, and the carry ledger records them as skipped. An entity at health ≤ 0 accepts no new instance. Hot reload keeps instances by status name, drops instances of removed kinds and applies changed parameters from the next tick; it never resets the tick phase.
- **Every impact runs its policies, whatever produced it.** The v1 producer gate retires: script damage evaluates in landing order at the next evaluation point — the in-tick evaluation that follows it, else a host frame-end evaluation before the end-of-frame removal pass, which republishes the owning pawn's health first. In-tick evaluations stop dropping script impacts. Connected clients discard script impacts unevaluated, as their damage is not authoritative. Script damage has no damager, so its `impact.source` grants skip as an absent source already does.
- **Fluids apply a status.** A fluid's optional `status` applies it every tick to each player pawn whose body is in that fluid at any depth, with applier `fluid.<name>` and no damager. The field is host-only gameplay data: it stays out of the replicated movement tuning, which clients need for prediction and this is not. Fluid damage is a short-duration status: lava with `durationMs` equal to `tickMs` stops one tick after the player leaves, slime with a long duration lingers.
- **Fluid statuses reach player pawns only.** Agents and corpses resolve gravity, not fluid (`entity_model.md` §7, Who resolves what), and an enemy that resolved fluid would also need nav that avoids it, which Epic 10 owns. A mod hurts an enemy in a fluid with an authored trigger and `applyStatus`.
- **Reads:** `impact.target.hasStatus(h)` and the brain-guard `hasStatus(h)` hold while any instance of `h` is live on that entity, mirroring `defense(h)` per scope as a Bool, as `impact.type(h)` is. `impact.status(h)` holds when this impact is a tick of `h`, so a policy that ignites on fire damage can refuse to re-ignite from its own burn ticks. `player.status.<name>` is an owner-private Number, the most time left across the pawn's instances of that kind in ms, absent when none; it changes the state-schema fingerprint, not `WIRE_VERSION`, as `player.defense.<name>` did.
- **Effects and verbs:** `impact.target.applyStatus(h)` and `impact.target.clearStatus(h)` join impact effects in the consequential lane; reaction subjects (`npcs()`, `players()`, `on.activators`, `on.player`) gain `.applyStatus(h)` and `.clearStatus(h)`, host-only like `.damage`. Clear removes every instance of that kind. A subject without health is skipped.
- **A status may declare attached presentation:** `presentation: { emitter?, sound? }`, an emitter built with the SDK `emitter()` builder and a looping sound, both following the afflicted entity while any instance of that kind is live. The owning player's own pawn gets the sound, not the emitter, as its first-person view hides its own avatar. Presentation is local: each machine attaches it from status presence, never from replicated emitters.
- **Status presence replicates per entity** as a bitset over status declaration order, in a new entity component payload, so co-op clients attach presentation to enemies and teammates. It bumps `WIRE_VERSION` and `SNAPSHOT_VERSION` (`networking.md` §Version gates). Declaration order must agree between peers, so status names join the mod compatibility digest.
- **The status stage's per-tick cost is bounded by construction:** it visits only entities carrying instances, allocates nothing per tick, and reports as its own CPU stage. A stress run proves the bound (Acceptance).
- **Non-goals:**
  - `onDamage` per-attack aggregate: demand-gated until a mod needs a per-attack reduction; shot-id threading to the chokepoint belongs to `drafts/E10--stealth-opener`.
  - Status parameters as expressions, status application chance, and a status that changes movement or fire (slow, stun, disarm). Damage over time is this brief's only effect; other effects are their own decisions.
  - `impact.source.applyStatus` (thorns), overlay-callback status reads, and teammate status on the HUD.
  - `.damage(amount, { status })` and `defineDamageType(matrix, { inflicts })`: those reserved doors stay reserved and empty. Application goes through one verb, `applyStatus`.
  - Mesh tint for afflicted entities: no per-entity tint exists outside the ghost-link look.
  - Fall damage and other environmental producers.

### Scripting surface
```ts
// scripts/damage-types.ts
export const fire = defineDamageType({ shield: 0.5, health: 1.25 });
export const acid = defineDamageType({ armor: 2 });

// scripts/statuses.ts — each named by its binding
import { defineStatus, emitter } from "postretro";
import { acid, fire } from "./damage-types";
export const burning = defineStatus({
  damage: 4, type: fire, tickMs: 500, durationMs: 3000,
  stacking: "stack", maxStacks: 3,
  presentation: { emitter: emitter({ sprite: "ember", rate: 24, lifetime: 0.6, velocity: [0, 1.5, 0] }), sound: "burn_loop" },
});
export const scalding = defineStatus({ damage: 10, type: fire, tickMs: 250, durationMs: 250 });
export const corroding = defineStatus({ damage: 2, type: acid, tickMs: 1000 }); // until cleared

// scripts/combat.ts
import { defineFluid, defineImpactEvent, defineMod, defineReaction, hasStatus, players } from "postretro";
export const ignite = defineImpactEvent({ tag: "enemy" }, (impact) => [
  { when: impact.type(fire).and(impact.status(burning).not()), do: [impact.target.applyStatus(burning)] },
  { when: impact.status(burning).and(impact.target.healthAfter.le(0)), do: [impact.target.playAnim("burn_death")] },
]);
export const cure = defineReaction("cure", [players().clearStatus(corroding)]);
// in a brain transition: flee while burning
const panic = { to: "flee", when: hasStatus(burning) };

export default defineMod({
  damageTypes: [fire, acid],
  statuses: [burning, scalding, corroding],
  fluids: [defineFluid({ name: "lava", viscosity: 0.6, status: scalding })],
  events: [ignite],
});
// HUD: read(player.status.burning) — ms left, absent when not burning
```
Luau mirrors every name, passing the status name explicitly as it does for damage types: `Postretro.defineStatus("burning", { ... })`.

## Acceptance
### Automated
**Ticks and credit**
- [ ] A hit that applies `burning` produces its first tick `tickMs` later and then every `tickMs`; each tick drains layers before health by the type's matrix and restarts layer recharge delays.
- [ ] A lethal tick latches death on its own tick and credits the applier: the contributor ledger names the applying hit's source id and the kill report credits the applier's player.
- [ ] A tick runs impact policies with `impact.source` as the applier and `impact.status(burning)` true; an `impact.source.grantHealth` row on a burn tick heals the applier; a non-tick hit reads `impact.status(burning)` false.
- [ ] Applier despawned before a tick: the tick still lands, credits the source id, and source grants skip.

**Stacking, timing and lifecycle edges**
- [ ] Eight pellets from one applier on one tick add one stack; two appliers on one tick create two instances that tick and credit independently.
- [ ] `"refresh"` resets time left without moving the tick phase; `"stack"` caps at `maxStacks` and multiplies damage; `"ignore"` leaves a live instance unchanged.
- [ ] A tick due on the expiry tick lands once, and no tick follows; an instance with `durationMs` omitted ticks until cleared.
- [ ] `durationMs` below `tickMs`, `tickMs` below one fixed tick, `maxStacks` outside [1, 64], a 65th status, an unknown key and a bad handle each reject load naming the key path; the boundary values load.
- [ ] Apply and clear on one frame: the later in body or stage order wins; clear removes every applier's instance.
- [ ] Death latch, respawn, level change, same-map restart and save each leave no instance; an entity at health ≤ 0 refuses a new instance; recovery by `setHealth` starts with none.
- [ ] A 17th instance replaces the one with the least time left.
- [ ] Hot reload keeps live instances and their phase, drops a removed kind, and applies a changed `damage` from the next tick.

**Script damage and policies**
- [ ] An enemy killed by script `applyDamage` from a trigger, a crossing, a `wait` tail and a UI press each runs its death policy, despawns and reports the kill in the same frames an in-tick kill does.
- [ ] A script impact that lands before a weapon hit in the same tick evaluates before that hit's impact, in landing order, and is not dropped.
- [ ] A connected client running a raw-tag `applyDamage` evaluates no policy.

**Fluids**
- [ ] A player wading ankle-deep in `lava` takes `scalding` ticks; leaving stops them one tick later; standing exactly on the surface takes none; a fluid without `status` applies none; an enemy in the fluid takes none.

**Reads, presentation and co-op**
- [ ] `hasStatus` holds in an impact policy and a brain guard while any instance is live and not after it ends; `player.status.burning` reads the most time left and is absent when none.
- [ ] Status presence reaches a connected client's entity record within one snapshot of application and clears within one snapshot of the last instance ending; a peer whose status declaration order differs fails the compatibility digest at join.
- [ ] The Scripting surface example installs as a `content/dev` script and its fixture passes in TS and Luau.

**Bounds**
- [ ] Stress: 1024 entities each holding four live instances tick within the plan of record's measured budget with zero heap allocation across the status stage (`testing_guide.md` §Resource bounds); an entity with no instances costs nothing.

### Manual
- [ ] Single player: ignite enemies with the dev incendiary weapon; burning embers and loop sound follow each one, stop on expiry, and a burn kill plays the authored death.
- [ ] Walk into dev lava: health drops while in it and stops just after leaving; the HUD shows the status.
- [ ] Co-op, two peers at the dev lag profile: the client sees host-ignited enemies and a burning teammate with embers and sound; the client's own burn shows on its HUD; kill credit goes to the player who ignited.

## Path
- **Precedents.** Defense layers are the component template: `DefenseLayersComponent`, `recharge_defense_layers`, `for_each_defense_layers_lent`, the spawn, respawn, carry (`CarriedField` ledger in `crates/netcode/src/seat.rs`) and hot-reload hooks (`plan_defense_layers_refresh`). Reads follow `defense(h)` in each scope with Bool typing from `impact.type(h)` and `attackTurnHeld`. Effects follow `grantLayer` from SDK lowering to `bind_effect` and `apply_effect`. The handle kind joins `HandleRef`. Owner-private slots follow `player.defense.<name>` (`replace_player_defense_slots`, the client `Unset` arm). The replicated payload follows ghost links' entity component payload. `research.md` maps every file.
- **Script-impact evaluation.** The in-tick takers (`evaluate_pending_in_registry[_with_overlay_sink]`) take the whole queue; make them evaluate script impacts instead of skipping them, and turn `discard_app_drain_pending` into a host-only evaluation that republishes pawn health. `research.md` §Script damage sites lists every site and its role.
- **Fluid identity.** The movement resolve drops the fluid index for a wading pawn; carry it out of the tick-start resolve beside immersion so the status stage reads it.
- **Shape and rival.** Chosen: one engine instance per status and applier, ticked in-tick. Rival: statuses as author `@state` counters plus a ticking reaction. Rejected because a reaction runs outside the tick, credits nobody and cannot replicate presence.
- **First slice.** `burning` applied by an impact policy, ticking through the chokepoint and killing with credit, single player. It falsifies the in-tick tick-and-credit premise before the surface, fluids, presentation and wire work.
- Files past ~800 lines this extends (`crates/sim/src/impact_policy.rs`, `crates/sim/src/sim/mod.rs`, `sdk/lib/data_script.ts`): split first, behavior-preserving, own commit.

## Open questions
- Exact placement of the attached emitter (entity origin vs. mesh center) and the sound's attenuation — **delegated**: the executor decides and reports it in the plan of record.
- Whether status presence rides every replicated entity or only health-bearing ones — **delegated**.

### Boundary inventory
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| status handle | `HandleRef::Status` | `{ "kind": "status", "name" }` | `defineStatus(opts)` / `defineStatus(name, opts)` | `Postretro.defineStatus(name, opts)` | n/a |
| manifest list | manifest `statuses` | `"statuses"` | `statuses` | `statuses` | n/a |
| fluid field | `FluidDescriptor.status` | `"status"` (handle) | `status` | `status` | n/a |
| has-status read | entity/brain input | `@status.<name>` | `hasStatus(h)` | `hasStatus(h)` | n/a |
| tick-of read | impact input | `@impact.status.<name>` | `impact.status(h)` | `impact.status(h)` | n/a |
| effects / verbs | impact effect + reaction primitive | `"applyStatus"`, `"clearStatus"`, args `{ status }` | `.applyStatus(h)`, `.clearStatus(h)` | `:applyStatus(h)`, `:clearStatus(h)` | n/a |
| HUD slot | owner-private slot family | `player.status.<name>` | `player.status.<name>` | `player.status.<name>` | n/a |
| source ids | ledger `source_id` | `fluid.<name>`, `script.applyStatus` | n/a | n/a | n/a |
| presence payload | new component payload | `status_presence` | n/a | n/a | n/a |

Both TS and Luau ship.

### Wire format
`status_presence` is one entity component payload in bitcode, as `WireMeshAnimationState` is: a single `u64`, bit *i* set while any instance of the *i*-th declared status is live. An entity with no live status sends no payload; removal follows the existing component-removal path. Status names, in declaration order, join `mod_compatibility_digest` so index *i* names one status on both peers.
