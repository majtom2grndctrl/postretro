# E16--status-effects-and-hazards

Brief · resumable · Epic 16 · reads: `context/lib/scripting.md` §2, §10.8, §11, §12 · `context/lib/entity_model.md` §Components, §5 Update Order, §7 Environment Volumes · `context/lib/networking.md` §Combat authority, §What gates, and what replicates instead, §Version gates · `context/lib/testing_guide.md` §Resource bounds · read at e7438238 · evidence: `research.md`

Builds on `plans/done/E16--damage-types-and-defense-layers` (typed damage, layers, the reserved status doors) and `plans/done/E16--impact-policy-substrate` (the producer gate this brief retires).

## Problem
The owner wants lava and slime that hurt, and burn, shock and corrode effects, as genre table stakes. None can be authored today, and damage that does not come from a weapon or AI hit skips authored consequences:
- **Script damage runs no impact policy.** The impact evaluator skips every impact the `applyDamage` reaction produces, by design of the v1 producer gate. An enemy killed by a scripted hurt volume or trap stays a zero-health corpse: no authored death, no despawn, no kill report. Set-pieces, a first-class PostRetro feature (`context/lib/index.md` §1), depend on that path.
- **Fluids cannot hurt.** A fluid declaration has movement and presentation fields only; `plans/done/E24--water-and-environment-volumes` left fluid damage as "a later field" on the fluid's own data.
- **No status mechanism exists.** The engine has no per-entity effect that deals damage over time, so DoT can only be faked with script `wait` loops.

When this is done, every impact runs its policies, whatever produced it. A mod declares statuses and applies them from impact policies, reactions and fluids. Every damaging status tick drains layers and health through the damage chokepoint with a type and a source, runs impact policies, and credits whoever applied it. Enemies can react to their own statuses. A HUD can show the local player's statuses, and players see statuses on others through attached particles and a per-tick sound, in co-op as well.

## Decisions
- **Every impact runs its policies, whatever produced it.** Script damage evaluates on the host where it lands, in landing order. Inside a fixed tick it evaluates right after its damage applies, as weapon and AI hits do. Outside a tick (a UI press, a player event's fire list, a crossing, `levelLoad`, a frame drain) it evaluates at whichever comes first: the start of the next tick, before anything reads health or statuses, or frame end, before the end-of-frame removal pass. Its effect on the next sweep and status stage never depends on unrelated hits or on how ticks pack into frames; a policy's live reads (`@state`, `player.health`) see whatever landed before it evaluates. Connected clients discard script impacts unevaluated, as their damage is not authoritative.
- **A status is an engine-owned noun with author-named kinds,** following defense layers (`plans/done/E16--damage-types-and-defense-layers`, `scripting.md` §11). `defineStatus` joins the binding-name sugar; `defineMod({ statuses: [...] })` registers kinds, a handle crosses as `{ kind: "status", name }`, and a bad reference fails where a bad damage-type reference fails on that surface. Load fails past 64 statuses in one mod.
- **Parameters are literals:** `damage` per tick per stack [0, 1e6], `type` (a damage-type handle; omitted takes the mod's default type), `tickMs` [1, 60000], `durationMs` [`tickMs`, 600000] or omitted for "until cleared", `stacking` and `maxStacks` [1, 64, default 1]. `tickMs` and `durationMs` round up to whole fixed ticks, as weapon activation timings do.
- **A `damage: 0` status is a marker** ("wet", "oiled") that other policies read for combos. Its ticks never enter the chokepoint, so they run no policy.
- **An instance is keyed by status and applier.** The applier is the applying impact's damager; with no damager it is the applying source id (`fluid.<name>`, `script.applyStatus`). Separate appliers keep separate instances, so each tick credits the right player in co-op, and `maxStacks` and time left are per applier: four players can each stack a target to the cap. An entity holds at most 16 instances; a new one when full replaces the instance with the least time left (`research.md` §Instance cap).
- **Stacking is per instance:** `"refresh"` (default) resets time left; `"stack"` adds one stack up to `maxStacks` and resets time left, and a tick deals `damage × stacks`. All applications by one applier to one target on one fixed tick count as one, so eight shotgun pellets or a rocket's hit plus its splash apply once. This settles `research/combat-events.md` §3's per-attack question without an aggregate moment.
- **Tick timing:** an instance first deals damage `tickMs` after it is created, then every `tickMs`. Refresh and stack never move the tick phase, so a fluid refreshing every tick still ticks. A tick due on the tick the instance's time runs out still lands, then the instance ends. An application made outside a fixed tick (a UI press, a `wait` tail, a frame-end policy) counts on the next fixed tick and applies at its start, after pending script damage evaluates, as trap requests latch.
- **Ticks are host-authoritative in-tick hits:** a status stage runs after Weapons and before the death sweep (`entity_model.md` §5), so a lethal tick latches and credits on its own tick. Each damaging tick enters the damage chokepoint with the status's type, the applier as damager and the applying hit's source id, so it drains layers, restarts layer delays, records the contributor, feeds brain damage memory and runs impact policies with `impact.source` as the applier. A due tick whose target is already at health ≤ 0 is skipped, so another hit's authored death plays alone. Clients predict no status and no tick (`networking.md` §Combat authority).
- **Statuses are transient:** they clear when the death latch sets, on respawn, and on level change, same-map restart and rejoin, and never enter carried state. A save records none, so a loaded save starts with none; writing a save leaves live instances ticking. An entity at health ≤ 0 accepts no new instance. Hot reload keeps instances by status name, drops instances of removed kinds and applies changed parameters from the next tick without moving the tick phase.
- **Fluids apply a status.** A fluid's optional `status` applies it every tick to each player pawn whose body is in that fluid at any depth. The field is host-only gameplay data and stays out of the replicated movement tuning, which exists for client prediction.
- **Fluid statuses reach player pawns only.** Agents and corpses resolve gravity, not fluid (`entity_model.md` §7, Who resolves what), and an enemy that resolved fluid would also need nav that avoids it, which Epic 10 owns. A mod hurts an enemy in a fluid with an authored trigger and `applyStatus`.
- **Reads:** `impact.target.hasStatus(h)` and the brain-guard `hasStatus(h)` hold while any instance of `h` is live on that entity, as Bool reads beside `defense(h)` in each scope. `impact.status(h)` holds when this impact is a tick of `h`, so a policy that ignites on fire damage can refuse to re-ignite from its own ticks. A status handle's `.active` is a Bool read of the local pawn's presence, as a layer handle's `.current` reads the local pawn's layer: on the host and in single player from the live component, on a connected client from its own pawn's replicated presence. It is not a replicated value, so the state-schema fingerprint never carries status names, and like `player.lifeState` it takes no `byPlayer`.
- **Effects and verbs:** `impact.target.applyStatus(h)` and `impact.target.clearStatus(h)` join impact effects in the consequential lane; reaction subjects (`npcs()`, `players()`, `on.activators`, `on.player`) gain `.applyStatus(h)` and `.clearStatus(h)`, host-only like `.damage`. Clear removes every instance of that kind. A subject without health is skipped.
- **A status may declare attached presentation:** `presentation: { emitter?, tickSound? }`. The emitter, built with the SDK `emitter()` builder, follows the afflicted entity while any instance of that kind is live; the owning player's own pawn gets no emitter, as its first-person view hides its avatar. `tickSound` is a one-shot sound at the entity every `tickMs` while presence holds, from when presence appears, played locally on every machine and not phase-locked to host ticks. Each machine attaches presentation from status presence, never from replicated emitters.
- **Status presence replicates per entity** in a new entity component payload, bumping `WIRE_VERSION` and `SNAPSHOT_VERSION` (`networking.md` §Version gates). A client maps it to its own statuses by the host's names, as fluids resolve presentation by name; a name the client does not declare shows nothing. Status names stay out of the mod compatibility digest: on a client a status is presentation only, and `networking.md` hashes only what cannot be replicated. After a host reload that reorders statuses, a client may show the wrong status until the new name table lands; the mis-map is cosmetic and brief.
- **The status stage allocates nothing for its own bookkeeping per tick,** and an entity with no instances costs nothing. The chokepoint and policy cost of its ticks is measured and reported, not gated.
- **Non-goals:**
  - `onDamage` per-attack aggregate: demand-gated until a mod needs a per-attack reduction; shot-id threading to the chokepoint belongs to `drafts/E10--stealth-opener`.
  - Status parameters as expressions: resistance already varies through the type matrix, because ticks are typed hits.
  - Application chance: the impact IR has no random source, and adding one is its own determinism decision.
  - A status that changes movement or fire (slow, stun, disarm): each touches prediction, so each is its own decision.
  - `impact.source.applyStatus` (thorns): `impact.target.applyStatus` covers every reported case; the source arm adds when a mod asks.
  - Overlay-callback status reads: overlay callbacks bind Number roots only.
  - Time left on the HUD and teammate status on the HUD: each needs a replicated value, which presence deliberately is not.
  - An entity-attached looping sound: the audio core refuses looping sounds with an entity anchor; Epic 12 owns it.
  - `.damage(amount, { status })` and `defineDamageType(matrix, { inflicts })`: those reserved doors stay reserved and empty (Path, Shape and rival).
  - Mesh tint for afflicted entities: no per-entity tint exists outside the ghost-link look.
  - Fall damage: Doom has none, Quake's is token, and Epic 11's traversal argues against punishing falls.

### Scripting surface
```ts
// scripts/damage-types.ts
export const fire = defineDamageType({ health: 1.25 });
export const acid = defineDamageType({ health: 0.75 });

// scripts/statuses.ts — each named by its binding
import { defineStatus, emitter } from "postretro";
import { acid, fire } from "./damage-types";
export const burning = defineStatus({
  damage: 4, type: fire, tickMs: 500, durationMs: 3000,
  stacking: "stack", maxStacks: 3,
  presentation: {
    emitter: emitter({ sprite: "ember", rate: 24, lifetime: 0.6, velocity: [0, 1.5, 0] }),
    tickSound: "burn_tick",
  },
});
export const scalding = defineStatus({ damage: 10, type: fire, tickMs: 250, durationMs: 250, presentation: { tickSound: "sizzle" } });
export const corroding = defineStatus({ damage: 2, type: acid, tickMs: 1000 }); // until cleared
export const wet = defineStatus({ damage: 0, tickMs: 500, durationMs: 5000 }); // marker

// scripts/combat.ts
import { defineFluid, defineImpactEvent, defineMod, defineReaction, hasStatus, players } from "postretro";
export const ignite = defineImpactEvent({ tag: "enemy" }, (impact) => [
  { when: impact.type(fire).and(impact.status(burning).not()).and(impact.target.hasStatus(wet).not()),
    do: [impact.target.applyStatus(burning)] },
  { when: impact.type(acid), do: [impact.target.clearStatus(wet)] },
  { when: impact.status(burning).and(impact.target.healthAfter.le(0)), do: [impact.target.playAnim("burn_death")] },
]);
export const cure = defineReaction("cure", [players().clearStatus(corroding)]);
// in a brain transition: flee while burning
const panic = { to: "flee", when: hasStatus(burning) };

export default defineMod({
  name: "statuses-demo", id: "example.statuses", version: "1",
  damageTypes: [fire, acid],
  statuses: [burning, scalding, corroding, wet],
  fluids: [defineFluid({ name: "lava", viscosity: 0.6, viewTint: [1, 0.35, 0.05, 0.8], muffle: 0.9, surfaceOpacity: 1, status: scalding })],
  events: [ignite],
  reactions: [cure],
});
// HUD widget: visibleWhen: stateEquals(burning.active, true) — shown while the local player burns
```

## Acceptance
### Automated
**Script damage and policies**
- [ ] An enemy killed by script `applyDamage` from a trigger, a teleport, a player event, a crossing, a `wait` tail, a UI press and `levelLoad` each runs its death policy, despawns and reports the kill in the same frames an in-tick kill does; a `levelLoad` kill evaluates on the reveal frame against that level's policies (P15).
- [ ] A trigger-bound lethal `applyDamage` with a `setHealth` recovery policy recovers before the sweep, with or without another hit on that tick, and `playerDied` never fires; a player-event script impact after tick N evaluates before tick N+1's sweep whether N+1 runs in the same frame or the next (P13, P14).
- [ ] A trigger-bound lethal `applyDamage` with a `setHealth` recovery policy on an enemy whose `burning` tick is due recovers before the status stage, and the tick lands, with or without another hit earlier that tick; a trigger-bound `applyDamage` whose policy clears `burning` leaves no tick either way (P30).
- [ ] A trigger binding that lists a lethal `applyDamage` with a `setHealth` recovery policy and then `applyStatus(burning)` on the same enemy leaves a `burning` instance; a second `applyDamage` in that list sees the recovered health (P31).
- [ ] A trigger-bound or teleporter-bound lethal `applyDamage` with a `setHealth` recovery policy, then a rocket on the same enemy that tick: the rocket hits the recovered enemy, deals damage, records its contributor and feeds damage memory, and its policies read `healthAfter` from the recovered health (P31).
- [ ] A trigger-bound `applyDamage` whose policy applies `burning`, and a UI-press `applyDamage` whose policy applies it, each make a brain guard on `hasStatus(burning)` hold on that tick, with or without a projectile hit earlier that tick; a UI press that deals lethal damage with a `setHealth` recovery policy and then applies `burning` to the same enemy leaves a `burning` instance on the next tick (P31, P32).
- [ ] A script impact that lands before a weapon hit in the same tick evaluates before that hit's impact, in landing order, and is not dropped.
- [ ] A frame-end policy on script damage to the local pawn reads `player.health` as post-damage health, and so does a policy on trigger-bound script damage evaluated in-tick, with or without another hit that tick, and one on player-event script damage evaluated at the next tick's start; an `impact.source.grantHealth` row on script damage grants nothing; a script-damaged enemy enters the damaged-enemy roster as an in-tick hit does (P16).
- [ ] A connected client running a raw-tag `applyDamage` evaluates no policy (regression guard).

**Ticks and credit**
- [ ] Each `burning` tick drains layers before health by the type's matrix and restarts layer recharge delays.
- [ ] A lethal tick latches death on its own tick and credits the applier: the contributor ledger names the applying hit's source id and the kill report credits the applier's player. With the applier despawned before the tick, the tick still lands, credits the source id, and source grants skip.
- [ ] A tick runs impact policies with `impact.source` as the applier and `impact.status(burning)` true; an `impact.source.grantHealth` row on a burn tick heals the applier; a non-tick hit reads `impact.status(burning)` false.
- [ ] A rocket kill and a due `burning` tick on one tick: the burn tick deals nothing and runs no policy, so the rocket's death policy alone plays (P6).
- [ ] A `wet` (`damage: 0`) instance ticks without entering the chokepoint: no impact policy, no layer delay restart, no contributor; `hasStatus` and presence see it (P24).
- [ ] A `burning` tick on an enemy updates its damage memory toward the applier, so a damage-recency guard holds and retaliation can select the applier.

**Stacking, timing and lifecycle edges**
- [ ] Eight pellets from one applier on one tick add one stack; two appliers on one tick create two instances that tick, stack to `maxStacks` and credit independently.
- [ ] `"refresh"` resets time left without moving the tick phase; `"stack"` caps at `maxStacks` and multiplies damage; a `"stack"` application at `maxStacks` adds nothing and still resets time left.
- [ ] `tickMs` and `durationMs` round up to whole fixed ticks: an unrefreshed `burning` (500 / 3000) applied by a hit lands its first tick `tickMs` later, then every `tickMs`, exactly six ticks, the last on its expiry tick and none after; `tickMs` 1 and 16 each tick every fixed tick at 60 Hz, and 17 ticks every second fixed tick (P5).
- [ ] A weapon hit that refreshes or stacks an instance on its expiry tick keeps it, lands that tick's damage at the phase it already had, and ticks on; an instance with `durationMs` omitted ticks until cleared (P2).
- [ ] An unguarded `"refresh"` policy that reapplies `burning` from its own tick on the expiry tick keeps that instance, with its tick phase unmoved, and the next tick follows one `tickMs` later; the scripting-reference recipe for an igniting policy guards with `impact.status(h)`, and its fixture shows the guarded burn expiring (P1).
- [ ] An applier who rejoins with a new pawn and reapplies `burning` creates a second instance; the old one credits only its source id until it ends (P23).
- [ ] Two appliers' `burning` instances both due on the lethal tick land in instance creation order; the first credits its applier's player (P7).
- [ ] A tick's own policy changes the instance set at once: a `burning` tick that clears `burning` leaves none, one that applies `corroding` leaves a `corroding` instance that does not tick this tick, and a `corroding` instance cleared by an earlier tick's policy on the same tick does not land (P8).
- [ ] Two `"stack"` applications from UI presses in two frames with no tick between add one stack, counted on the next fixed tick, and the instance's first tick lands `tickMs` after that tick (P10).
- [ ] A `players().applyStatus(burning)` from a player event after tick N, and one from a policy on player-event script damage, each count on tick N+1: they dedupe with N+1's applications by that applier, and the first tick lands `tickMs` after N+1 (P27).
- [ ] A remote client's incendiary hit that waited on fire authorization on tick N applies `burning` counted on tick N: it dedupes with that applier's other tick-N hits and its first tick lands `tickMs` after N (P33).
- [ ] A UI press that applies `burning` and a crossing that clears it in the same frame leave no instance on the next tick; the reverse order leaves one; a trigger-bound clear on the tick a latched application applies removes it before its tick (P25).
- [ ] A latched application is dropped when, by the tick it would apply, its target has despawned, died, respawned or rejoined, or hot reload removed its kind; a frame-end policy that despawns and applies to one target leaves nothing; level change and same-map restart discard every latched application (P26).
- [ ] A `burning` applied by a policy on script `applyDamage` keys to `script.applyDamage`, and one applied by a policy on a `scalding` tick keys to `fluid.lava`; each is a separate instance from a `script.applyStatus` burn, and its ticks credit that source id and no player (P29).
- [ ] `durationMs` below `tickMs` or above 600000, `tickMs` below 1 or above 60000, `damage` outside [0, 1e6], `maxStacks` outside [1, 64], a 65th status, an unknown key and a bad `type` handle each reject load naming the key path; the boundary values load.
- [ ] A status handle not listed in `statuses` fails where a bad damage-type reference fails on that surface: load fails on a fluid `status` and a brain-guard `hasStatus`, install drops that one impact policy or reaction, and a HUD `.active` binding warns once at tree build.
- [ ] A status with `type` omitted ticks as the mod's `defaultDamageType`, or untyped ×1 when the mod declares none.
- [ ] Clear removes every applier's instance. A trigger-bound `clearStatus` on the tick an instance is due removes it before its tick lands; a clear from a player event or a frame drain after that tick leaves the tick landed (P9).
- [ ] Death latch (the in-tick sweep and the post-tick remote-hit sweep), respawn, level change, same-map restart, rejoin and loading a save each leave no instance; a checkpoint save mid-burn leaves the burn ticking; an entity at health ≤ 0 refuses a new instance; a deferred `setHealth` recovery after the latch starts with none, while an immediate recovery before the sweep keeps its instances (P11, P12).
- [ ] A 17th instance replaces the one with the least time left; among equals the oldest goes; an until-cleared instance counts as the most time left; the new instance always enters (P18).
- [ ] Hot reload keeps live instances and their next due tick, drops a removed kind, and applies a changed `damage` or `type` from the next tick; a changed `tickMs` spaces the ticks after that next one; a changed `durationMs` leaves time left unchanged until the next application; a lowered `maxStacks` clamps stacks on the next tick (P19).

**Fluids**
- [ ] A player wading ankle-deep in `lava` takes `scalding` ticks; after leaving, exactly one more tick lands within `tickMs`, then none; standing exactly on the surface takes none; a fluid without `status` applies none; an enemy in the fluid takes none; re-entering lava on a lingering instance's last tick keeps it and its phase (P4).
- [ ] A remote pawn in `lava` with no command for 30 ticks keeps taking `scalding` ticks; a pawn respawned out of lava takes none before its next command (P17).
- [ ] A fluid `status` leaves the replicated fluid table at name and viscosity.

**Reads, presentation and co-op**
- [ ] `hasStatus` holds in an impact policy and a brain guard while any instance is live and not after it ends; `burning.active` is true while the local pawn has a live `burning` instance, on the host and on a connected client, and false after it ends; a `burning.active` read in a player-event condition, a reaction a player event fires, or an interaction-pair expression rejects as a local-only read does, and `burning.active` has no `byPlayer` in TS or Luau.
- [ ] Each snapshot's presence bits equal host status presence at the tick it serializes, so presence reaches a connected client within one snapshot of application and clears within one snapshot of the last instance ending; a status live for less than one snapshot interval may never show (P21).
- [ ] A client that declares the same statuses in a different order shows the host's status by name; a host status the client does not declare shows nothing; declaring statuses changes neither the compatibility digest nor the state-schema fingerprint, and a host hot reload that adds a status keeps every client participating with its replicated state; after a host hot reload that adds or reorders statuses, a connected client shows each host status by its new name from the first snapshot after the new `statusNames` installs, without rejoining (P20).
- [ ] A client joining while an enemy burns shows nothing for presence that arrives before its status name table installs, then shows the burn from the first snapshot after install (P22).
- [ ] While `burning` presence holds on an enemy, the host and a connected client each attach one `ember` emitter at the entity and request one anchored `burn_tick` one-shot one `tickMs` after presence appears and every `tickMs` after, and stop both when presence clears; a pawn's own machine requests its tick sound but attaches no emitter to it, while every other machine attaches one; no emitter crosses the wire (P28).
- [ ] A peer one `WIRE_VERSION` behind is refused at handshake, a snapshot one `SNAPSHOT_VERSION` behind is rejected, and the tuning payload epoch advances.
- [ ] A connected client creates no instance, lands no tick and applies no `applyStatus` or `clearStatus` from a UI press or crossing; its health moves only by snapshot.
- [ ] `players().applyStatus` skips a subject without health and warns.
- [ ] The Scripting surface example installs as a `content/dev` mod in TS and Luau, type-checks with `tsc --noEmit` including `emitter()`, and its fixture shows `burning`, `scalding`, `corroding` and `wet` behaving as declared.

**Bounds**
- [ ] Stress: 1024 entities each holding four live instances, including instances that expire and are reapplied during the run, a full entity taking a 17th, and four pawns refreshed by a status fluid every tick. With `damage: 0` instances, the status stage allocates nothing across the run. With damaging instances, the plan of record reports per-tick time and allocations, counting the chokepoint call and its context as chokepoint and policy cost. The stage reports as its own CPU stage (`testing_guide.md` §Resource bounds), and an entity with no instances costs nothing.

### Manual
- [ ] Single player: ignite enemies with the dev incendiary weapon; embers follow each one with a sound every tick, stop on expiry, and a burn kill plays the authored death.
- [ ] Walk into dev lava: it sizzles every tick, health drops while in it and stops just after leaving; the dev HUD shows its status indicator (one widget per status) while `scalding` or `burning` is live.
- [ ] A dev trigger trap kills an enemy: the authored death plays and the corpse despawns.
- [ ] Co-op, two peers at the dev lag profile: the client sees host-ignited enemies and a burning teammate with embers and tick sounds; the client's own burn shows on its HUD; kill credit goes to the player who ignited.

## Path
- **Slice order** follows distance from PostRetro's core: script damage, then fluid hazards, then elemental application.
  - **First slice: script-damage evaluation,** its own commit (`research.md` §Routing). The in-tick takers (`evaluate_pending_in_registry[_with_overlay_sink]`) take the whole queue; make them evaluate script impacts instead of skipping them, evaluate in-tick script damage right after it applies at each in-tick site (Triggers, teleporter bindings), add a host evaluation at tick start for off-tick script damage, and turn `discard_app_drain_pending` into a host-only evaluation that republishes pawn health first, in both the frame loop and the headless driver (`observability/driver.rs`). `research.md` §Script damage sites lists every site and its role.
  - **Second slice: status engine with `scalding` lava,** single player. The movement resolve drops the fluid index for a wading pawn; carry it out of the tick-start resolve beside immersion so the status stage reads it.
  - **Third slice: `burning` applied by an impact policy,** ticking and killing with applier credit. `ImpactDispatch` carries the damager but neither the source id nor a status kind today; add both, so `applyStatus` records the source and `impact.status(h)` has data to read.
  - Then the surface, reads, presentation and wire work.
- **Precedents.** Defense layers are the component template: `DefenseLayersComponent`, `recharge_defense_layers`, `for_each_defense_layers_lent` (it lends only `&EntityRegistry`, so a stage that runs damage and policies mid-walk needs its own shape), and the spawn, respawn and hot-reload hooks (`plan_defense_layers_refresh`). Reads follow `defense(h)` in each scope with Bool typing from `impact.type(h)` and `attackTurnHeld`. Effects follow `grantLayer` from SDK lowering to `bind_effect` and `apply_effect`. The handle kind joins `HandleRef`. The replicated payload follows ghost links' entity component payload; the name table follows the fluid and life-state tables in the tuning payload. The client-derived status read follows `player.lifeState`: a local-only slot a client fills from its own pawn's replicated state through the host's name table; the local-player-slot guard (`is_local_player_slot`) is catalog-built and must learn the per-mod family. `emitter()` is exported at runtime but missing from `sdk/types/postretro.d.ts`; add its declaration. `research.md` maps every file.
- **Shape and rival.** Chosen: the mod applies statuses through `applyStatus` in impact policies, reactions and fluids. Rival: the predecessor's sketch, `defineDamageType(matrix, { inflicts })`, where the engine applies a status on every hit of that type. Rejected because a policy can gate on anything the impact scope reads — target tag, `broke`, health, other statuses, the tick-of read — while `inflicts` fixes one rule per type and would still need policies for every other case; it also builds the status-type cycle the predecessor flagged, since a status names its type. The cost of the chosen shape is that a naive "fire applies burning" policy reapplies itself from its own ticks; the `impact.status(h)` read is its guard, and the docs recipe shows it.
- Files past ~800 lines this extends (`crates/sim/src/impact_policy.rs`, `crates/sim/src/sim/mod.rs`, `sdk/lib/data_script.ts`): split first, behavior-preserving, own commit.

## Open questions
- Exact placement of the attached emitter (entity origin vs. mesh center) and the tick sound's attenuation — **delegated**: the executor decides and reports it in the plan of record.
- Whether status presence rides every replicated entity or only health-bearing ones — **delegated**.

### Boundary inventory
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| status handle | `HandleRef::Status` | `{ "kind": "status", "name" }` | `defineStatus(opts)` / `defineStatus(name, opts)` | `Postretro.defineStatus(name, opts)` | n/a |
| manifest list | manifest `statuses` | `"statuses"` | `statuses` | `statuses` | n/a |
| presentation | status descriptor | `"presentation": { "emitter", "tickSound" }` | `presentation: { emitter, tickSound }` | `presentation = { emitter, tickSound }` | n/a |
| fluid field | `FluidDescriptor.status` | `"status"` (handle) | `status` | `status` | n/a |
| has-status read | entity/brain input | `@status.<name>` | `hasStatus(h)` | `hasStatus(h)` | n/a |
| tick-of read | impact input | `@impact.status.<name>` | `impact.status(h)` | `impact.status(h)` | n/a |
| effects / verbs | impact effect + reaction primitive | `"applyStatus"`, `"clearStatus"`, args `{ status }` | `.applyStatus(h)`, `.clearStatus(h)` | `:applyStatus(h)`, `:clearStatus(h)` | n/a |
| HUD read | local-only slot, client-derived | slot `player.status.<name>` (never replicated) | `<handle>.active` | `<handle>.active` | n/a |
| source ids | ledger `source_id` | `fluid.<name>`, `script.applyStatus` | n/a | n/a | n/a |
| presence payload | new component payload | `status_presence` | n/a | n/a | n/a |
| host status name table | tuning payload entry | `statusNames` | n/a | n/a | n/a |

Both TS and Luau ship.

### Wire format
`status_presence` is one entity component payload in bitcode, as `WireMeshAnimationState` is: a single `u64`, bit *i* set while any instance of the host's *i*-th declared status is live. An entity with no live status sends no payload; removal follows the existing component-removal path. The host's status name table rides the JSON tuning payload as `statusNames`, an array of strings in declaration order, beside the fluid table; an empty array means no statuses. Adding it advances the tuning payload epoch, as the fluid and life-state tables did. Bit *i* names the host's *i*-th entry.
