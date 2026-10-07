# sdk-addressing-model

Brief · compact · reads: `context/lib/scripting.md` §10.5–§10.8, §12 · `context/lib/networking.md` §Game-logic-owned apply invariant · read at 2bc52d471

## Problem
A requested capability, raised by the owner while drafting `E16--player-events`. Scripts address entities in ways the spelling doesn't distinguish:
- `world.query` returns map entities as a snapshot taken when `setupLevel` runs.
- Live sets are split across `enemies({tag})`, `spawner({tag})` and plain tag strings, even though spawners are map entities. NPCs a spawner creates carry no tags, so no tag reaches them.
- A sequence cannot hold a live set, so a live command after a `wait` has to go through `fire(reaction)`.
- Commands are spelled two ways: as methods on the target (`m.start()`, `impact.source.grantAmmo(…)`) or as free functions that take it (`damage(target, n)`).
- "Enemy" names a relationship, not a kind of entity; a friendly NPC can turn hostile.

`E16--player-events` needs a player group and a spelling rule for event sources, and is paused for both. When done:
- Map entities come only from `getMapEntities`, as members the author inspects and addresses one by one.
- Entities whose membership changes (NPCs, including those a monster closet releases, and players) are addressed as groups. A group resolves whenever a command takes effect, sequence steps included.
- Every SDK command is a method on its target.

## Decisions
- **Members versus groups.** `getX` gets members now: an array the author inspects in JS and addresses one by one. A noun is a group: an opaque value with no members or length, resolved by the engine. Members are the only form the build-time light pass sees, and the only form that allows per-member authored arguments, because the IR has no iteration (`plans/done/E16--resource-grant-chokepoint`). Members never convert to groups; their baked ids are already generation-checked and warn-skip when stale.
- **Map entities are members only.** `world.query` becomes `getMapEntities(kind, { tag? })` over every map-placed kind (Boundary inventory). It returns map-placed instances only; a light or emitter carried by a spawned NPC never appears, so membership is fixed at install and a group would add nothing. Called outside a level's data context, as in a mod start script, it raises an error naming the call. `spawner` becomes a query kind, and a spawner member exposes its snapshot fields and `fire()`. This replaces the fire-time-tag spawner handle (`plans/done/E18--spawner-and-closet-containment`). The `world` namespace dissolves, and gravity moves to `getGravity()` and `setGravity()`.
- **Groups exist only where membership changes.**
  - `npcs({ tag? })` covers brain-driven characters that aren't players, whatever their current faction sentiment.
  - `players()` covers every player pawn bound to a seat: it skips a pawn whose seat is in a disconnect hold, and in single player it reaches the local pawn. This matches `E16--player-events`, where a held player is unobserved.
  - A group resolves on the machine draining the command, against its own registry, in an order that is the same for the same inputs.
  - This keeps `plans/done/E18--enemy-group-handle`'s fire-time model and renames it: `enemies` becomes `npcs`, and `updateEnemyState` becomes `updateNpcState`, wire included. Hostility belongs in a future filter, not a kind name.
- **A spawned NPC carries its spawner placement's tags, as if placed there.** A closet's output is then reachable by the same `npcs({ tag })` as its map-placed residents. `progress` counts kills only among the entities carrying its tag at install, which keeps its existing meaning: total captured at load, later spawns neither raise it nor count. This reverses the untagged runtime spawns of `plans/done/E18--spawner-and-closet-containment`, whose reason was protecting `progress` totals; membership now does that.
- **Every SDK command is a method on its target, following `impact.source.grantAmmo(…)`.** The free target-taking verbs `damage`, `grantHealth`, `grantAmmo`, `addSlot`, `armTrigger` and `disarmTrigger` retire, along with plain tag-string targets, with no shims. Reference content migrates where its hand-written target now has a verb: `applyDamage` on players. Fixtures that exercise the raw wire path stay raw, such as trigger-fanout-fixture's tag arming. Each target exposes exactly the verbs its kind supports (Boundary inventory). `players()` takes over the multi-player grant that `plans/done/E16--resource-grant-chokepoint` made tag-only.
- **A group command is a legal sequence entry, resolved when its step runs.** This reverses `plans/done/E18--timed-reaction-steps`' rule that id-keyed and tag-keyed steps cannot share a step list. A subject-token command is legal only before any `wait`. After one, E18's install check still drops the reaction, because the fire context does not survive the wait. Steps after a `wait` run on the host only, including member steps over map kinds, so a client never sees them.
- **Trigger events.**
  - In `setupLevel`, a trigger member receives events: `t.on("enter" | "exit", fire)`, keyed by that volume.
  - A mod-global trigger event is declared before any level exists, so it is a standing rule keyed by tag: `defineTriggerEvent({ tag, event, fire, levels? })`, accepted only in `ModManifest`.
  - A mod-global event and a level member event that resolve to the same volume, edge and reaction bind once, with a warning. Running the reaction twice would double its effects and restart any `wait` in it. This extends today's compose-time dedupe of identical tag-keyed descriptors to resolved bindings.
  - `onTriggerEvent` retires. `on.trigger` and `on.activators` keep their meaning and gain methods.
- **Spelling rule for sources.** A source that fires once per member or group member is spelled `.on(…)` on that target and publishes a subject token named for its kind. Tag-keyed declarations (`defineTriggerPool`, `defineImpactEvent`, `progress`) and `onStateCrossing` keep their spelling; the crossings redesign settles them.
- **Placement: SDK, descriptor parsing and dispatch.** The net wire is unchanged, and `NetworkId` never reaches scripts. NPC and player group commands run on host and single player only, through an explicit role check, not through replicated copies lacking data; a client holds its own player pawn. On a connected client, they apply nothing and log nothing above debug, as a client `addSlot` does today. Every machine runs the same reactions, so a host-only command reaching a client is normal content, not an error. Map members resolve on every machine from their local install.
- **Non-goals.**
  - `players().on(…)` and `on.player`: `E16--player-events` adds them on this surface. `research.md` §Impact on E16--player-events lists that brief's edits.
  - `.on` on NPC groups, and per-entity conditions over non-player sets: no consumer. `research.md` §Doors.
  - Mod-global commands over map entities. No mod-global reaction commands one today. A consumer gets either a manifest-only tag-keyed command form mirroring `defineTriggerEvent`, or mod-attached level scripts (their own brief). `research.md` §Doors.
  - System reactions (`updateState`, `playSound`, …) as sequence entries: `E16--player-events` owns it, because it already decides where its presentation steps run.
  - `gameState` replacing `getGameState()`: it lifts the `scripting.md` §5 ban, so it goes to the scripting-API session.
  - Renaming the rest of the "enemy" vocabulary (`damagedEnemies`, enemy overlays, `networking.md` wording): presentation and docs, for a follow-up brief.
  - Typed builders for primitives that have none today (`setFogScatter`, `setAnimationState`, …). This gap predates the brief and is not about addressing. Their raw tag-keyed descriptors stay valid wire data.
  - Damaging tagged entities that carry health but are neither NPCs nor players: no content does it. Such a kind joins when one appears.
  - A spawned-by relationship (`npcs({ spawnedBy: s })`) for when tags are too coarse: `research.md` §Doors.

### Scripting surface
```ts
import { defineReaction, getMapEntities, npcs, players, fire, wait } from "postretro";
import type { TriggerEventParams } from "postretro";
import { onStateCrossing, updateState } from "postretro/ui";
import { closetStore } from "./closet-store";

const raiseAlarm = defineReaction("closet.raiseAlarm", updateState(closetStore.alarm, 1));
const patchUp = defineReaction((on: TriggerEventParams) => on.activators.grantHealth(25));
const resupply = defineReaction("closet.resupply", players().grantAmmo("shells.buck", 8));

export function setupLevel() {
  const door = getMapEntities("mover", { tag: "closet_door" });              // members, fixed at install
  const closets = getMapEntities("spawner", { tag: "closet" });
  const plate = getMapEntities("trigger", { tag: "closet_reveal_plate" });
  const alarmLights = getMapEntities("light", { tag: "closet_alarm" })
    .sort((a, b) => a.position.x - b.position.x);                            // inspect in JS
  const closet = npcs({ tag: "closet" });       // group: placed residents and whatever the spawners release

  const alarmLight = defineReaction("closet.alarmLight", {
    sequence: alarmLights.flatMap((l, i) => l.pulse({ min: 0.3, max: 1.0, periodMs: 400 + i * 50 })),
  });
  const reveal = defineReaction("closet.timedReveal", {
    sequence: [
      ...fire(raiseAlarm),
      ...wait(800, { interruptible: true }),
      ...door.flatMap((m) => m.start()),
      ...closets.flatMap((s) => s.fire()),
      closet.update({ aggro: true }),            // whoever exists when this step runs
      closet.damage(5),
      ...fire(resupply),
    ],
  });

  return {
    reactions: [reveal, raiseAlarm, alarmLight, patchUp, resupply],
    triggerEvents: plate.flatMap((t) => [t.on("enter", [reveal]), t.on("exit", [patchUp])]),
    crossings: [onStateCrossing(closetStore.alarm, { above: 0 }, [alarmLight])],
  };
}

// start-script.ts: mod-global, no level loaded, so a standing rule keyed by tag
// triggerEvents: [defineTriggerEvent({ tag: "faction_sentiment_story", event: "enter", fire: [storyBeat] })]
```
Luau mirrors it with colon calls: `Postretro.getMapEntities("mover", { tag = "closet_door" })`, `Postretro.npcs({ tag = "closet" }):update({ aggro = true })`, `t:on("enter", { reveal })`, `on.activators:grantHealth(25)`.

## Acceptance
### Automated
**Members**
- [ ] Every content map's build-time light-membership output is byte-identical before and after the rename.
- [ ] arena-lights, switch-demo, coop-two-button-puzzles and trigger-fanout-fixture (TS and Luau) emit byte-identical manifest data before and after.
- [ ] `getMapEntities("npc")` and `getMapEntities("transform")` fail to compile in TS and raise an error naming the call in Luau. A query with no match returns an empty array.
- [ ] `getMapEntities` called in a mod start script raises an error naming the call, in both runtimes. The same call in `setupLevel` succeeds.
- [ ] A spawner member's `fire()` spawns from that spawner only, not from a sibling spawner carrying the same tag. This holds when name-fired, after a `wait`, and in a trigger's tick before any `wait`. Two `fire()` steps on one member spawn two batches (research A3, A5).
- [ ] A trigger member's `on("enter", …)` fires for that volume only, not for a sibling volume carrying the same tag.
- [ ] A sequence holding a light member step and a group step reserves the light's build-time membership exactly as it does without the group step (research A12).
- [ ] `getMapEntities("light")` and `getMapEntities("emitter")` exclude a light or emitter carried by a spawned NPC.

**Groups**
- [ ] `npcs({ tag: "x" }).damage(n)` skips a player pawn tagged "x". This holds when name-fired, after a `wait`, and in a trigger's tick before any `wait`. `npcs()` with no tag reaches every NPC (research A1).
- [ ] `players()` and tagless `npcs()` commands bind to a trigger edge, both as a primitive body and as a step before any `wait`, and apply in that trigger's tick. No binding logs a missing-tag warning (research A2).
- [ ] `players().grantHealth(n)` credits every player pawn once; `on.activators.grantHealth(n)` credits only that fire's activators.
- [ ] Group matches reach the handler in the same order on every run with identical inputs, including after a despawn frees a slot that a later spawn reuses.
- [ ] During a disconnect hold, `players()` skips that player's pawn for every verb, and reaches it again after reclaim. In single player, `players()` reaches the local pawn.

**Spawned NPCs**
- [ ] An NPC spawned by a spawner tagged `x` carries `x`, and `npcs({ tag: "x" })` reaches it. A spawner with no tags spawns untagged NPCs.
- [ ] A `progress` keyed by `x`, with N entities tagged `x` at install, fires at its threshold over those N only. Kills of NPCs spawned later carrying `x` neither count nor raise the total.
- [ ] Each of these fails to compile in TS and raises an error naming the call in Luau: `.length` or `.map` on a group, `damage("boss", 10)`, and a verb the kind lacks, such as `npcs().fire()`. In Luau, `#group`, `group[1]` and `group.length` raise an error naming the call; none returns 0 or nil.

**Sequences**
- [ ] A sequence of an interruptible `wait` followed by an NPC group step includes an NPC spawned during the wait and excludes one despawned during the wait. With zero matches, the step is a debug no-op. A `players()` step after a `wait` reaches a player who joined during the wait and skips one whose seat was released (research A6).
- [ ] `[s.fire(), npcs().update({ aggro: true })]` aggroes every NPC that `s` just spawned, in the same drain or tick. This holds when name-fired, in a trigger's tick, and after a `wait` (research A4).
- [ ] A subject-token step after a `wait` still drops its reaction at install, with an error naming it. A `players()` or `npcs()` step in the same position installs and lands (research A7).
- [ ] A mover member step followed by a group step on one drain runs in authored order. An Exit that cancels the wait runs neither.
- [ ] In a trigger-fired sequence, a group or `on.activators` step before any `wait` applies within the trigger's tick.

**Trigger events and sources**
- [ ] A mod-global `defineTriggerEvent` binds every volume carrying its tag in each level its `levels` selector matches, and fires as `onTriggerEvent` did.
- [ ] A tag-keyed trigger event returned from `setupLevel` is rejected at load with a warning naming the level script; the volume-keyed events beside it install.
- [ ] A `defineTriggerEvent` binds nothing in a level its `levels` selector excludes. A volume-keyed `{ trigger, event, fire }` in `ModManifest` is rejected at load with a warning naming the manifest; the tag-keyed events beside it install.
- [ ] An interruptible `wait` bound only through a member `t.on("enter", …)` installs. The exit edge is derived for that volume only, and leaving it cancels the tail. A sibling volume with the same tag neither fires nor cancels. On a `once` volume, the reaction is dropped (research A8).
- [ ] On one trigger edge, bound work runs brush first, then mod-global `defineTriggerEvent`, then the level member `on`, each in authored order (research A9).
- [ ] After a mod hot reload recomposes the active sets, a level member's `on("enter", …)` still fires for its volume, and only for it (research A10).
- [ ] A mod-global `defineTriggerEvent` and a level `t.on` that bind the same reaction to the same volume and edge run it once per edge, with one warning naming both.
- [ ] `on.trigger.disarm()` disarms the volume that fired; `on.trigger.arm()` re-arms it.
- [ ] `getGravity()` and `setGravity(v)` behave as `world.getGravity` and `world.setGravity` did, including the warning and no-op on a non-finite value.

**Roles**
- [ ] On a connected client, in a reaction with no `wait`, `npcs(…).update(…)` and `players().grantHealth(…)` apply nothing, not even to the client's own pawn, and log nothing above debug. The same reaction on the host applies to every match. A step after a `wait` never runs on the client.
- [ ] On a connected client, a member step in the same reaction as a skipped group step still applies (research A11).

**Wire**
- [ ] A sequence entry or primitive descriptor is rejected at parse, naming the reaction, in both runtimes, if it carries both `id` and `kind`, or a `kind` other than `npc` or `player`. A raw kindless descriptor still reaches the same entities as before: `{ primitive: "applyDamage", tag: "x" }`, and the hand-written descriptors left in anim-demo-reaction, fog-pulse-demo and trigger-fanout-fixture. A manifest naming `updateEnemyState` is rejected as an unknown primitive.
- [ ] Grep and diff gate: `WIRE_VERSION` is unchanged, and `NetworkId` appears in no scripting crate, no `sdk/` file and no generated typedef.

**Surface**
- [ ] The Scripting surface example runs as a `content/dev` script replacing closet-reveal. It runs on a map whose closet NPCs and closet spawner carry `closet`. After the wait lands:
  - the map-placed closet NPCs and the NPCs the spawner released are aggroed and damaged;
  - the spawner has spawned its count;
  - every player has received the ammo.

  Its TS and Luau twins produce byte-identical wire data.
- [ ] Regression guard, a grep gate. None of these remains in `content/`, `sdk/`, `docs/` or `context/lib/`:
  - `world.query`, `world:query`, `world.getGravity`, `world.setGravity`, `world:getGravity`, `world:setGravity`;
  - `enemies(`, `spawner(`, `updateEnemyState`, `onTriggerEvent`, `armTrigger(`, `disarmTrigger(`;
  - any match for `(damage|grantHealth|grantAmmo|addSlot)\(\s*["'@]`.

  `content/` also has no hand-written `applyDamage` descriptor targeting players.

### Manual
- [ ] Host plus client playtest of closet-reveal and coop-two-button-puzzles: doors, lights, NPC aggro and the spawner behave as before, and the client sees the host's result.

## Path
- **Group lowering.**
  - The primitive descriptor gains `kind` (`"npc"` or `"player"`) beside an optional `tag`. `dispatch_primitive` resolves the kind's component through `EntityRegistry::query_by_component_and_tag`.
  - A descriptor without `kind` keeps today's `Transform` scan, for raw data only.
  - `players()` resolves seat-bound pawns through `EntityRegistry::seat_for_pawn`.
  - `BoundTarget::Tag` cannot carry a kind or an absent tag today, so the trigger path needs a group target (research A1, A2).
  - The client role check reuses the `owner_slot_writes_enabled` silence pattern.
- **Sequence arm.**
  - `SequenceTarget` gains a group arm. `sequence_steps_from_js` and its Luau twin accept a `kind`-bearing entry.
  - `dispatch_sequence` hands the entry to `ReactionPrimitiveRegistry::dispatch_tagged`. `bind_sequence_step` maps it to `BoundTarget::Tag`.
  - `partition_direct_reaction` classifies it by primitive, and `collect_membership` skips it.
  - Rival considered: desugar group entries at SDK time into auto-named reactions reached by `fire()`. That needs no engine change, but each entry becomes a queued dispatch instead of a step in authored order.
- **Spawner member.**
  - `fire()` becomes a sequenced id step that calls `spawn_from_spawner_targets` with the one id. `SpawnContext` gating is unchanged.
  - Today an id-targeted spawn in a trigger's tick warns and skips (`trigger_commands.rs`), so the bound path needs an id arm.
  - The spawn path in `spawner.rs` already builds a synthetic map entity per NPC with empty tags; it copies the spawner's tags instead.
  - `spawner` needs a `worldQuery` arm and snapshot shape (`entity_world_primitives.rs`, `WORLD_QUERY_COMPONENTS` in `light_membership.rs`).
- **Progress.** The progress tracker captures the id set carrying its tag at install, and counts kills by membership in that set rather than by tag.
- **Raising outside a level.** Primitive scope is advisory today (`scripting.md` §3), so `getMapEntities` needs its own data-context check.
- **Validation.** `reaction_validation.rs` V2 and V3 read only tag-keyed trigger events. The volume-keyed form must count toward Enter provenance (research A8).
- **Light pass.** The build-time light pass skips a whole sequence when one step lacks an `id`. Group entries must not trip that check (research A12).
- **Trigger member.** The trigger-event descriptor gains a volume-keyed form. `bind_event` in `trigger_bindings.rs` already turns a tag into ids at install, so the volume-keyed form skips that query.
- **Members API.** `getMapEntities` replaces `sdk/lib/world.{ts,luau}` and maps kinds to today's component strings. The `worldQuery` FFI primitive stays internal. Touch the special case in `typedef/mod.rs` and the build-time stub `query_world_json` in `light_membership.rs`.
- **Methods.**
  - Group and token objects carry their verbs. The `IMPACT_SOURCE` handle is the precedent.
  - The Luau prelude currently lifts `DATA_SCRIPT_FIELDS` (`luau_prelude.rs`) to bare globals. Its list changes to the new builders.
  - No lifted name may shadow a Luau builtin.
- **Consumers.** `research.md` §Consumers lists every site to migrate in one pass.
- **First slice.** Closet-reveal with an NPC group step after the wait, on the host, plus a test that spawns an NPC during the wait. This falsifies the sequence arm and its scheduler residual, the riskiest piece.
- **Split first.** `reaction_dispatch.rs`, `trigger_bindings.rs`, `data_script.ts` and `data_script.luau` are past ~800 lines. Split only the ones this work extends, behavior-preserving, each in its own commit.

## Open questions
- Which component marks the `npc` kind (the brain component or a dedicated marker) — **delegated**.
- Whether the raw `worldQuery` primitive stays visible in the generated typedefs — **delegated**.

## Boundary inventory
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Map members | `worldQuery` primitive (internal) | n/a (FFI result) | `getMapEntities(kind, { tag? })` → handle array | `Postretro.getMapEntities(kind, { tag = … })` | n/a |
| Map kinds | component kinds | n/a | `mover`, `trigger`, `light`, `fog`, `emitter`, `spawner` | same | n/a |
| Groups | tag target with kind, on primitive descriptors and as a sequence target | `kind: "npc" \| "player"`, `tag?` | `npcs({ tag? })`, `players()` | `Postretro.npcs(…)`, `Postretro.players()` | n/a |
| NPC state primitive | `updateNpcState` handler | `"updateNpcState"` (was `"updateEnemyState"`) | `npcs(…).update(fields)` | `:update(fields)` | n/a |
| Spawner member fire | sequenced spawner step | `{ id, primitive: "spawnFromSpawner" }` | `s.fire()` | `s:fire()` | n/a |
| Trigger events | trigger-event descriptor, keyed by volume or tag | `triggerEvents[]`: `{ trigger, event, fire }` in a level; `{ tag, event, fire, levels? }` in the mod | `t.on(event, fire)`; `defineTriggerEvent({ tag, event, fire, levels? })` | `t:on(…)`; `Postretro.defineTriggerEvent({ … })` | n/a |
| Gravity | existing primitives | n/a | `getGravity()`, `setGravity(v)` | same | n/a |

| Target | Verbs |
|---|---|
| mover member | `start`, `stop`, `reverse`, `goToPathNode`, `setSpinRate`, `setBlockPolicy` |
| light member | `pulse`, `fade`, `flicker`, `colorShift`, `sweep` |
| fog member | `pulse`, `fade`, `flicker`, `pulseSaturation`, `fadeSaturation` |
| trigger member | `arm`, `disarm`, `on` |
| spawner member | `fire`; snapshot fields |
| emitter member | none; snapshot fields only |
| `npcs(…)` | `update`, `damage` |
| `players()` | `damage`, `grantHealth`, `grantAmmo`, `addSlot` |
| `on.activators` | `damage`, `grantHealth`, `grantAmmo`, `addSlot` |
| `on.trigger` | `arm`, `disarm` |

## Wire format
Manifest JSON only. The net wire is unchanged and `WIRE_VERSION` does not move.
- **Primitive descriptor.** It may carry `kind` beside `tag`. With `kind`, resolution covers only that kind; without it, today's `Transform` scan applies.
- **Sequence entry.** Either `{ id, primitive, args }`, as today, or `{ primitive, kind, tag?, args }`. An entry carrying both `id` and `kind` is rejected at parse, naming the reaction.
- **Trigger event.** In a level script it is `{ trigger: <id>, event, fire }`. In `ModManifest` it is `{ tag, event, fire, levels? }`. Each is rejected where the other belongs.
- **Primitive rename.** `updateEnemyState` becomes `updateNpcState`.
