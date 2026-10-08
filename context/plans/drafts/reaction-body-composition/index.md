# reaction-body-composition

Brief · compact · reads: `context/lib/scripting.md` §10.4, §12 · `context/lib/networking.md` §Presentation events vs. replicated state · read at f29992e78

## Problem
A requested capability, raised by the owner while updating `E16--player-events` after `plans/done/sdk-addressing-model`. A reaction cannot bundle different kinds of effect:
- System reactions (`playSound`, `flashScreen`, `updateState`, `loadLevel`, …) cannot be `sequence` entries. "Damage the player, wait 300 ms, play a hiss" needs the hiss in a second reaction reached by `fire(r)`, and `fire` drops the emitter and the fire context.
- A body holds one effect or a `{ sequence }`. "Heal the activators, disarm the plate, play a sound" needs `{ sequence: [...] }` ceremony and spread operators. A plain array is rejected at load.

When done:
- Any system reaction is a sequence step.
- An array body, plain or returned from the `(on) => …` tracer, means a sequence.
- After a `wait`, a presentation step plays on the same machines it would have played on before the wait.

## Decisions
- **Every system reaction is a legal step.** A step does what the same reaction does as a body, on the machine that runs it. Presentation, state writes, sentiment, UI-stack and game-flow verbs all qualify; a closed subset would be a second list to learn (`scripting.md` §10.4 already treats them as one namespace). A step with no `id`, `kind` or `target` is a system step. Parse rejects one that names a primitive that is not a system reaction, naming the reaction and step, in both runtimes.
- **Wire: a system step is the body descriptor `{ primitive, args }`.** This follows the group and subject-token precedent, where an entry is the same descriptor as its body (`plans/done/sdk-addressing-model`). Rust gains a payload-free `System` sequence target. No `@system` sentinel, because the builders already emit this shape. A system step carrying `tag` or `onComplete` is rejected at parse.
- **When it lands.** A step lands when the same body would:
  - A `setState` step before any `wait` in a trigger-bound reaction binds in-tick, like a `setState` body (`plans/done/E18--trigger-event-fanout`).
  - Every other system step queues at the walk and applies at the frame-end system drain. Authored order holds among system steps; a member or group step earlier in the same walk has already applied.
  - IR-valued `setState` steps bind at install like `setState` bodies (`research.md` §Dispatch today).
- **A wait changes when a presentation step plays, never where.** Presentation means `playSound`, `rumble`, `flashScreen`, `vignette` and `screenShake`.
  - From a source the host mirrors (`levelLoad`, crossings over shared state, named gameplay events), a connected client parks the tail too and lands only its presentation steps; the host's copy lands the rest, so the client logs nothing above debug.
  - From a local-only source (UI presses, crossings over the machine's own owner-private or local slots), the client also lands only its presentation steps. No machine runs the rest, so the client warns once per reaction, naming it and the dropped step, keeping the diagnostic E18 shipped (`plans/done/E18--timed-reaction-steps` O43).
  - A post-wait `fire(r)` on a client dispatches `r` through the same filter, so a shared sting reached through `fire` plays where its inline twin would.
  - From a host-only source (trigger events), the host lands the tail. Its presentation reaches the audience a pre-wait step would have reached, which today is the host.
  - Every non-presentation step after a wait stays host-only, keeping E18's rule that delayed consequences are host-authoritative and replicated (`plans/done/E18--timed-reaction-steps` Invariants).
  - This adopts E18's named follow-up, client tails restricted to presentation, and reverses its client-refusal default (owner ruling). Rivals: `research.md` §Where a post-wait presentation step should play.
- **The fire context reaches system steps before a wait and not after.**
  - Before a wait, a step inherits the fire it runs under. `at: on.emitter` resolves, dispatch inputs bind, and E16's presentation routing applies.
  - After a wait, install drops a reaction whose system step reads `at: on.emitter` (V4a, unchanged) or a `setState` value that reads a dispatch input (V4a, extended).
  - V4b extends to a `fire` target's `setState` steps.
  - No author-visible token survives a wait, but `E16--player-events`' internal target player, which routes presentation only, does. This brief lands after E16: in a `players().on` reaction, an inline post-wait presentation step reaches the target player's machine over E16's message kind. The scheduler instance retains the target player across the wait. E16's `fire` rule and its plain-read rejection are unchanged.
- **E18 interactions are unchanged.** An interruptible wait's Exit cancel and a re-fire restart cover system steps in the tail. On a client, where no trigger runs, an interruptible wait lands as non-interruptible and logs nothing above debug. Teardown, suspend and hot reload drop client tails as they drop host ones.
- **`fire(r)` stays as the reuse idiom.** Use it for named, sourceless beats that several reactions, brush KVPs or a TS/Luau pair share. It is no longer needed to wrap a single effect. Nothing deprecates it. An inline step keeps the emitter, the subject routing and authored order, all of which `fire` loses (`research.md` §`fire(r)` after this brief). Reference content drops its single-effect wrappers.
- **Multi-effect bodies (separable).**
  - An array body is SDK sugar for `{ sequence }` with the same entries. The wire, parser, trigger partition, scheduler and light pass are unchanged, so it is not a second dispatch mechanism. It needs system steps to lower.
  - Both the plain body and the tracer's return accept an array or a `{ sequence }`.
  - Arrays may hold `wait` and `fire`, since a wait-free rule would protect nothing.
  - Nested arrays flatten at any depth, in array bodies and in `sequence` lists alike, so spreads become optional. Already-flat content lowers byte-identically.
  - An empty array, including Luau's empty table, is an inert empty sequence.
  - An entry that is not a step (`progress`, a nested `{ sequence }`, a reaction handle, a primitive with `onComplete`) throws at author time, naming `defineReaction`.
  - Rivals: `research.md` §Array bodies.
- **Placement.** The work sits in the SDK builders, both descriptor parsers, sequence dispatch and the trigger partition, and the scheduler's client role. Nothing changes on the net wire and `WIRE_VERSION` stays put; post-wait player-event presentation reuses the message kind E16 adds.
- **Non-goals.**
  - Whose screen a trigger-fired effect belongs to: `coop-trigger-screen-effects`. Whatever it decides covers post-wait steps under the rule above.
  - Member light and fog steps after a wait on clients (`research.md` §Doors).
  - Subject tokens after a wait, and a cancel verb: E18's rules stand.
  - Multi-effect impact policies, which have their own `Effect` vocabulary.

### Scripting surface
```ts
import { defineReaction, getMapEntities, npcs, players, wait } from "postretro";
import type { TriggerEventParams } from "postretro";
import { onStateCrossing, playSound, screenShake, updateState } from "postretro/ui";
import { closetStore } from "./closet-store";

// Several effects, one body. An array is a sequence; the tracer may return either.
const medkit = defineReaction("closet.medkit", (on: TriggerEventParams) => [
  on.activators.grantHealth(25),
  on.trigger.disarm(),
  playSound("sfx/test_tone"),
]);

export function setupLevel() {
  const door = getMapEntities("mover", { tag: "closet_door" });
  const closets = getMapEntities("spawner", { tag: "closet_spawner" });
  const plate = getMapEntities("trigger", { tag: "closet_reveal_plate" });
  const medkitPlate = getMapEntities("trigger", { tag: "closet_medkit" });
  const alarmLights = getMapEntities("light", { tag: "closet_alarm" });
  const closet = npcs({ tag: "closet" });

  // Trigger-fired, so it runs on the host. System steps sit beside member and group steps.
  const reveal = defineReaction("closet.timedReveal", [
    updateState(closetStore.alarm, 1),         // in the firing tick; replicates
    wait(800, { interruptible: true }),        // no spread needed
    door.map((m) => m.start()),                // nested arrays flatten
    closets.map((s) => s.fire()),
    closet.update({ aggro: true }),
    closet.damage(5),
    players().grantAmmo("shells.buck", 8),     // was fire(resupply)
    playSound("fixtures/door_open"),           // the host, as a pre-wait step here would be
  ]);

  // Crossing-fired, so it runs on every machine: each one plays the post-wait sound.
  const alarm = defineReaction("closet.alarm", {
    sequence: [
      alarmLights.map((l, i) => l.pulse({ min: 0.3, max: 1.0, periodMs: 400 + i * 50 })),
      screenShake(4, 300),
      wait(1200),
      playSound("sfx/test_tone"),
    ],
  });

  return {
    reactions: [reveal, alarm, medkit],
    triggerEvents: [
      ...plate.map((t) => t.on("enter", [reveal])),
      ...medkitPlate.map((t) => t.on("enter", [medkit])),
    ],
    crossings: [onStateCrossing(closetStore.alarm, { above: 0 }, [alarm])],
  };
}
```
Luau mirrors it: `Postretro.defineReaction("closet.medkit", function(on) return { on.activators:grantHealth(25), on.trigger:disarm(), UI.playSound("sfx/test_tone") } end)`, with nested tables flattening the same way.

## Acceptance
### Automated
**System steps**
- [ ] Every system reaction in the SDK installs as a sequence step in both runtimes and has the same effect as the same reaction used as a body. Steps fire by name, from `levelLoad`, by crossing and by trigger.
- [ ] Parse rejects a step with no target that names a non-system primitive (`applyDamage`), naming the reaction and step, in both runtimes. A step carrying `tag` or `onComplete` is rejected too. The same `playSound` step parses.
- [ ] A name-fired `[m.start(), updateState(a, 1), playSound(x), updateState(b, read(a))]` starts the mover during the walk, then writes `a` and then `b` (reading 1), and plays `x`, all in the same frame (research S1).
- [ ] Trigger-bound `[updateState(alarm, 1), playSound(x), wait(800), playSound(y)]`: `alarm` is written in the firing tick, `x` plays at that frame's drain, and `y` plays at landing (research S2).
- [ ] An IR-valued `setState` step evaluates at the drain like the same body, never logging "not bound at level install". Targeting a per-owner slot, it is rejected at install as a body would be.
- [ ] A mover-event reaction `[playSound(x, { at: on.emitter }), wait(300), playSound(y)]` installs and plays `x` at the emitter and `y` unpositioned. Moving the `at` step after the wait drops the reaction at install, naming it (research S6).
- [ ] A crossing reaction `[wait(300), updateState(s, read(on.rising))]` is dropped at install, naming it. The same step before the wait installs and writes the crossing's direction (research S7).
- [ ] `fire(r)`, where `r`'s `setState` step reads `on.rising`, drops the firing reaction at install, at any step position (research S8).
- [ ] An Exit that cancels an interruptible wait runs none of the tail's system steps. An accepted re-fire restarts the body and replays its pre-wait system steps once (research S5).
- [ ] A sequence holding a light member step and a system step reserves the light's build-time membership exactly as it does without the system step.

**Where tails land**
- [ ] Loopback host plus client, `levelLoad = [wait(500), npcs().update({ aggro: true }), playSound(x), m.start()]`. The client plays `x` once and applies neither other step. The host applies all three. Neither logs above debug about the client tail (research S3).
- [ ] Loopback, a `player.health` crossing below 25 with `[vignette(…), wait(600), playSound(x)]`. Each machine plays `x` only for its own crossing, and the host sends the client nothing (research S4).
- [ ] Loopback, a trigger-fired `[wait(300), flashScreen(…)]`. The host presents it and the client does not, matching the same step before the wait.
- [ ] On a client, a post-wait `setState` or `loadLevel` step does not run, and a post-wait `fire(r)` lands only `r`'s presentation steps. On the host the same steps all run.
- [ ] On a client, a UI press firing `[playSound(click), wait(300), returnToFrontend()]` plays the click, drops the return and warns once naming the reaction and step; a second press warns no further. A crossing over shared state with the same shape warns nothing above debug.
- [ ] On a client, a crossing firing `[wait(300), fire(sting)]` plays `sting`'s sound exactly as `[wait(300), playSound(…)]` does.
- [ ] Unloading the level, suspending or hot-reloading on a client drops its parked tails, and nothing lands into the next level.
- [ ] Grep and diff gate: `WIRE_VERSION` is unchanged, and no Presentation message kind is added.
- [ ] Loopback host plus two clients, a `players().on` reaction for client A's player holding `[on.player.damage(5), wait(300), flashScreen(…)]`: the flash presents on A only, not the host or client B, as the same step before the wait does. `[wait(300), fire(flash)]` in the same reaction is still dropped at install.

**Multi-effect bodies**
- [ ] `defineReaction(name, [a, [b, wait(5)], c])` and `defineReaction(name, { sequence: [a, b, ...wait(5), c] })` emit byte-identical wire data, in TS and Luau alike (research S9).
- [ ] A tracer returning an array and one returning `{ sequence }` install. Each array form fires as its `{ sequence }` equivalent: by name, by trigger before any wait, and after a wait.
- [ ] `{ sequence: [door.map((m) => m.start())] }` lowers byte-identically to the spread form. Every shipped content script emits byte-identical manifest data before and after.
- [ ] An empty array body, and an empty Luau table body, installs as an inert reaction with no warning above debug.
- [ ] Putting `progress`, a nested `{ sequence }`, a reaction handle or an `onComplete` primitive in an array throws at author time, naming `defineReaction`, in both runtimes. A handle's error suggests `fire`. TS rejects each at compile time.
- [ ] `[on.activators.grantHealth(25), on.trigger.disarm(), playSound(x)]` on a trigger Enter heals only that fire's activators and disarms that volume within the tick, then plays `x` at the drain.

**Surface**
- [ ] The Scripting surface example runs as a `content/dev` script that replaces closet-reveal, on a map that adds a `closet_medkit` trigger. Its TS and Luau twins produce byte-identical wire data. On the host the reveal behaves as before, and the medkit heals, disarms and plays.

### Manual
- [ ] Host plus client playtest of the example. The client hears the alarm's post-wait sound on its own machine; the host hears the reveal's door sound; the medkit plate behaves on both.

## Path
- **Parse.** `sequence_steps_from_js` and `sequence_steps_from_lua` gain a `System` arm ahead of the required-`id` fallthrough. `sequence_primitives_are_valid` consults `SystemReactionRegistry` for it.
- **Dispatch.** `dispatch_sequence` takes the system registry and calls the `dispatch_system_primitive` path for `System` steps, inside the caller's `SystemCommandFireContext`.
- **Trigger partition.** `bind_sequence_step` binds a `System` `setState` through the same `bind_command` path as `bind_primitive`; other system steps join the residual.
- **Bindings and validation.** `SystemReactionIrBindings::rebuild` walks sequence steps. Pass A V4a and Pass B V4b read system steps (`reaction_validation.rs`).
- **Client tails.** `ReactionScheduler` gains a client mode that enrolls only tails containing a presentation step, lands only those steps, and is advanced by the connected-client tick. Rival: forward host tails over E16's Presentation message kind. It is right for one-player audiences and wrong for own-state crossings (research).
- **SDK.**
  - System builders return a `SystemReactionStep` type assignable to both a body and a step.
  - `ReactionBody` gains nested-array entries.
  - `defineReaction` and the Luau mirror flatten and lower arrays before computing the auto id.
  - `light_membership.rs` `runtime_sequence_step_shape_is_valid` accepts system steps.
- **Player-event tails.** `InstanceKey`'s origin is `(EntityId, PlayerId)` keyed to a trigger; a `players().on` instance has no trigger entity, so the origin needs a player-only form carrying E16's target player.
- **First slice.** Loopback `levelLoad = [wait(500), playSound(x)]` landing on both machines. This falsifies the client tick seam, the riskiest piece; parsing alone proves little.
- **Split first.** `reaction_scheduler.rs`, `reaction_validation.rs`, `system_reactions.rs`, `data_script.luau` and `main.rs` are past ~800 lines. Split only the ones extended, behavior-preserving, each in its own commit.

## Open questions
- Whether client tails reuse `ReactionScheduler` with a role mode or get a sibling presentation-only scheduler — **delegated**.

## Boundary inventory
Both runtimes ship every row.
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| System step | `SequenceTarget::System` (payload-free) | sequence entry `{ primitive, args }`: no `id`, `kind`, `target`, `tag` or `onComplete` | any system builder unspread, typed `SystemReactionStep` | same builders, `UI.playSound(…)` etc. | n/a |
| Step type | n/a | n/a | `SystemReactionStep` joins `SequenceStep`; system builders return it | Luau step type mirror | n/a |
| Array body | n/a (lowered by the SDK) | `{ sequence: [...] }`, flattened | `ReactionBody` gains `ReactionEntry[]` with nesting; plain and tracer forms | `defineReaction(name, { … })` table body or tracer return | n/a |
| Client presentation tail | scheduler client role | n/a (net wire unchanged) | n/a | n/a | n/a |

## Wire format
Manifest JSON only; the net wire and `WIRE_VERSION` do not change.
- **Sequence entry** gains a system form: `{ primitive, args }` with no `id`, `kind` or `target`. It is rejected at parse, naming the reaction, if `primitive` is not a system reaction, or if it carries `tag` or `onComplete`.
- **Array bodies** never reach the wire; the SDK emits `{ sequence }`.
