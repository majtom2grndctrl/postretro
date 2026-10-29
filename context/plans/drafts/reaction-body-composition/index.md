# reaction-body-composition

Brief · resumable · reads: `context/lib/scripting.md` §10.4, §12 · `context/lib/networking.md` §Presentation events vs. replicated state · read at 7eab3e3e7

## Problem
The owner requested this capability. It surfaced while building the ghost-links demo (`content/dev/scripts/ghost-links.{ts,luau}`). A reaction cannot bundle different kinds of effect. A system reaction (`playSound`, `setGhostAudio`, `updateState`, `loadLevel`, …) is never a `sequence` step, so `echo.swell` wraps one `setGhostAudio` call that the trigger fires beside `echo.reveal`. A body holds one effect or a `{ sequence }`, so every reaction carries `{ sequence: [...spreads] }`, and the Luau twin hand-rolls flatten helpers. A beat that waits cannot share a body with what fires at once, so the split crossing and the return each fire two reactions. A `levelLoad` tail is refused on every client with a warning. When done:
- Any system reaction is a sequence step.
- An array body means a sequence, whether plain or returned from the `(on) => …` tracer.
- After a `wait`, a machine-local step lands on the same machines it would have landed on before the wait.

## Decisions
- **Every system reaction is a legal step.** A step does what the same reaction does as a body, on the machine that runs it. A step with no `id`, `kind` or `target` is a system step. Parse rejects one whose primitive is not a system reaction, naming the reaction and the step, in both runtimes. A closed subset would be a second list to learn, since §10.4 treats all system reactions as one namespace.
- **Wire: a system step is the body descriptor `{ primitive, args }`.** Its entry is the same descriptor as its body, following the group and subject-token precedent (`plans/done/sdk-addressing-model`). Rust gains a payload-free `System` sequence target and no sentinel. Parse rejects a system step carrying `tag` or `onComplete`.
- **When it lands.** A step lands when the same body would land.
  - A pre-wait `setState` step in a trigger-bound reaction binds in-tick (`plans/done/E18--trigger-event-fanout`).
  - Every other system step queues at the walk and applies at the frame-end system drain, in authored order among system steps.
  - IR-valued `setState` steps bind at install, as bodies do.
- **Machine-local is one shared predicate.** A system step is machine-local when its `SystemReactionClass` is `Presentation` or `MachineLocal`, cell writes included, or when it is a `SlotWrite` to a slot that does not replicate. The rule moves out of `player_events::validate`'s private `machine_local_effect` into one public function. This brief's client filter, E16's install rule and `ui-timed-reactions` all call it. `HostConsequence` verbs, `setGhostAudio` and sentiment among them, are never machine-local; clients see their effect by replication.
- **A wait changes when a step lands, never where.**
  - A landing restores the origin its instance parked under: `FireOrigin::EveryMachine`, or `HostOnly` with `FireSubject::Trigger`, `Teleport` or `PlayerEvent`. The seat is re-resolved from the instance key's `PlayerId`. A host landing of an every-machine tail therefore presents locally, as its pre-wait copy does, and no sound plays twice.
  - Member light, fog and emitter steps in a host landing still record as host-only overrides and reach clients by replication, so `plans/done/coop--networked-by-default` AC10 stands.
  - Tails from sources every machine runs and the host mirrors (`levelLoad`, crossings over shared state, movement and weapon events) park on connected clients too. A client lands only the tail's machine-local steps and logs nothing above debug. The host lands every step.
  - For a local-only source (a UI press, a crossing over the machine's own owner-private or local slots), a client lands only its machine-local steps and warns once per reaction, naming it and the dropped step. This keeps E18's O43 diagnostic.
  - On a client, a post-wait `fire(r)` dispatches `r` through the same filter.
  - Host-only sources (triggers, teleports, player events, AI, death and mover events) land on the host. Their presentation reaches the audience the same step would reach before the wait.
  - Every non-machine-local step after a wait stays host-only (E18 Invariants). This widens E18's named client-tail follow-up from presentation to machine-local steps and reverses its client-refusal default (owner ruling).
- **Audience across a wait.** A pre-wait step carries `audience` exactly as a body does. A literal audience (`"everyone"`) survives a wait and routes as the pre-wait step would. A token audience after a wait stays dropped at install by V4a. This amends `scripting.md` §12 ("a presentation builder is never a sequence step, so no audience token reaches a `wait` landing") and `plans/done/coop--networked-by-default` Decision 11. Promotion updates the lib doc.
- **The fire context reaches system steps before a wait and not after.** A pre-wait step inherits its `SystemCommandFireContext`, so `at: on.emitter` resolves, dispatch inputs bind, and the origin routes its presentation. After a wait, install drops a reaction whose system step reads `at: on.emitter`, a token audience or a dispatch input (V4a, which now reads system steps). V4b extends to a `fire` target's system steps. Only the landing's restored origin survives, and no token does. E16's `fire` rule and its plain-read rejection are unchanged.
- **E16's install rule walks steps.** A `players().on` reaction holding a non-forwardable machine-local step is rejected, as the same body is today.
- **E18 interactions are unchanged.** An interruptible wait's Exit cancel and a re-fire restart cover system steps. On a client an interruptible wait lands as non-interruptible and logs nothing above debug. Teardown, suspend and hot reload drop client tails as they drop host ones.
- **`fire(r)` stays the reuse idiom.** It serves named beats shared by several reactions, brush KVPs or a TS/Luau pair, and nothing deprecates it. A single effect no longer needs a wrapper. An inline step keeps the emitter, the origin and authored order (`research.md` §`fire(r)` after this brief).
- **Array bodies are SDK sugar for `{ sequence }`.** The wire, parser, partition, scheduler and light pass are unchanged.
  - The plain body and the tracer's return each accept an array or a `{ sequence }`.
  - Arrays may hold `wait` and `fire`.
  - Nested arrays flatten at any depth, in arrays and `sequence` lists alike. Already-flat content lowers byte-identically.
  - An empty array, Luau's empty table included, is an inert empty sequence.
  - A non-step entry throws at author time, naming `defineReaction`: `progress`, a nested `{ sequence }`, a reaction handle or `onComplete`.
  - Rivals: `research.md` §Array bodies.
- **Placement.** The work touches:
  - the SDK builders and `defineReaction` in both runtimes;
  - both descriptor parsers;
  - sequence dispatch and the trigger partition;
  - install validation;
  - the scheduler, which gains a client role and keeps each instance's origin.
  
  The net wire and `WIRE_VERSION` do not change, and no message kind is added.
- **Non-goals.**
  - Waits in UI-fired reactions belong to `ui-timed-reactions`. Until it lands, a UI press's tail follows the local-only rule above.
  - Subject tokens after a wait, and a cancel verb: E18's rules stand.
  - Multi-effect impact policies, which have their own `Effect` vocabulary.

### Scripting surface
The ghost-links demo, rewritten (excerpt; the unchanged reactions take the same shapes):
```ts
import { defineReaction, fire, getMapEntities, npcs, wait } from "postretro";
import { setGhostAudio } from "postretro/audio";
import { playSound } from "postretro/ui";

export function setupLevel() {
  const echoes = getMapEntities("ghostLink", { tag: "vault_echo" });
  // One body: link steps, then the bus swell, a system step. No spreads, no wrapper.
  const reveal = defineReaction("echo.reveal", [
    echoes.map((l) => l.arm()),
    echoes.map((l) => l.setColorFilter({ tint: [1.0, 0.4, 0.3], opacity: 0.6, periodMs: 1500 })),
    setGhostAudio({ volume: 0.9, lowPassHz: 5000, periodMs: 1500 }),
  ]);

  const northDoor = getMapEntities("mover", { tag: "echo_closet_a1" });
  const northCloset = defineReaction("closet.a1", [
    northDoor.map((door) => door.start()),
    npcs({ tag: "echo_monster_a1" }).update({ aggro: true }),
  ]);
  // Every machine runs levelLoad. The host opens the closet; each machine sounds the klaxon once.
  const northTimer = defineReaction("levelLoad", [
    wait(15000),
    fire(northCloset),                 // reuse: barrier_approach fires it too
    playSound("sfx/test_tone"),
  ]);

  // The crossing into copy B: one body where two reactions used to fire.
  const southDoor = getMapEntities("mover", { tag: "echo_closet_a2" });
  const closetsB = getMapEntities("mover", { tag: "echo_closet_b" });
  const splitLanded = defineReaction("split.landed", [
    southDoor.map((door) => door.start()),
    npcs({ tag: "echo_monster_a2" }).update({ aggro: true }),
    wait(3000),
    closetsB.map((door) => door.start()),
    npcs({ tag: "echo_monster_b" }).update({ aggro: true }),
  ]);

  return {
    reactions: [reveal, northCloset, northTimer, splitLanded /* , … */],
    triggerEvents: [
      ...getMapEntities("trigger", { tag: "echo_plate" }).map((t) => t.on("enter", [reveal])),
      ...getMapEntities("trigger", { tag: "barrier_approach" }).map((t) => t.on("enter", [northCloset])),
    ],
    teleportEvents: getMapEntities("teleporter", { tag: "split_to_b" }).map((t) => t.on("teleport", [splitLanded])),
  };
}
```
`return.settle` and `wave.start` merge into one body the same way. Luau mirrors the demo with nested tables in place of its `append`/`each` helpers, e.g. `Postretro.defineReaction("closet.a1", { doorSteps, Postretro.npcs({ tag = "echo_monster_a1" }):update({ aggro = true }) })`, where `doorSteps` is a table of step tables.

## Acceptance
### Automated
**System steps**
- [ ] Every system reaction in `SystemReactionKind::ALL` installs as a sequence step in both runtimes. Each has the same effect as the same reaction used as a body. Steps fire by name, from `levelLoad`, by crossing, by trigger and by teleport.
- [ ] Parse rejects a step with no target that names a non-system primitive (`applyDamage`), naming the reaction and step, in both runtimes. A step carrying `tag` or `onComplete` is rejected too. The same `playSound` step parses.
- [ ] Name-fired `[m.start(), updateState(a, 1), playSound(x), updateState(b, read(a))]` behaves as follows, all in the same frame (research S1):
  - the mover starts during the walk;
  - `a` is written, then `b`, which reads 1;
  - `x` plays.
- [ ] Trigger-bound `[updateState(alarm, 1), playSound(x), wait(800), playSound(y)]` writes `alarm` in the firing tick, plays `x` at that frame's drain and plays `y` at landing (research S2).
- [ ] An IR-valued `setState` step evaluates at the drain like the same body and never logs "not bound at level install". If it targets a per-owner slot, install rejects it as it would a body.
- [ ] A mover-event reaction `[playSound(x, { at: on.emitter }), wait(300), playSound(y)]` installs, plays `x` at the emitter and plays `y` unpositioned. With the `at` step after the wait, install drops the reaction, naming it (research S6).
- [ ] A crossing reaction `[wait(300), updateState(s, read(on.rising))]` is dropped at install, naming it. With the step before the wait, it installs and writes the crossing's direction (research S7).
- [ ] `fire(r)` drops the firing reaction at install, at any step position, when `r`'s `setState` step reads `on.rising` (research S8).
- [ ] An Exit that cancels an interruptible wait runs none of the tail's system steps. An accepted re-fire restarts the body and replays its pre-wait system steps once (research S5).
- [ ] A sequence holding a light member step and a system step reserves the light's build-time membership exactly as it does without the system step.
- [ ] A `players().on` reaction `[on.player.damage(5), showDialog(d)]` is rejected at install, naming it. `[on.player.damage(5), playSound(x)]` installs and plays `x` on the event player's machine.

**Where tails land**
- [ ] Loopback host plus client, `levelLoad = [wait(500), npcs().update({ aggro: true }), playSound(x), m.start()]`. The client plays `x` and applies neither other step. The host applies all three. Neither machine logs above debug about the tail (research S3).
- [ ] Loopback host plus client, `levelLoad = [wait(500), playSound(x)]`, then `[wait(500), fire(r)]` where `r` plays `x`. In each case the client hears `x` exactly once and the host hears it exactly once.
- [ ] Loopback host plus client, `levelLoad = [wait(500), l.fade({ from: 1, to: 0.2, periodMs: 500 }), playSound(x)]` on a client-local light `l`. The light change reaches the client by replication, as it does without the system step, and `x` plays once per machine.
- [ ] Loopback, a `player.health` crossing below 25 with `[vignette(…), wait(600), playSound(x)]`. Each machine plays `x` only for its own crossing, and the host sends the client nothing (research S4).
- [ ] Loopback, the client's player fires a trigger bound to `[wait(300), flashScreen(…), playSound(y)]`. The flash presents on the client only and `y` plays on every machine once, matching the same steps before the wait.
- [ ] Loopback host plus two clients, a `players().on` reaction for client A's player holding `[on.player.damage(5), wait(300), flashScreen(…)]`. The flash presents on A only. `[wait(300), fire(flash)]` in the same reaction is still dropped at install.
- [ ] Audience after a wait:
  - In a trigger-bound `[wait(300), flashScreen(…, { audience: "everyone" })]`, the flash presents on every machine once.
  - With `audience: on.activators` after the wait, install drops the reaction, naming it.
- [ ] On a client, a post-wait `setState` on a replicated slot, a `setGhostAudio` or a `loadLevel` does not run, and a post-wait `fire(r)` lands only `r`'s machine-local steps. On the host the same steps all run, and the client sees the slot and the ghost-bus change by replication.
- [ ] Loopback host plus client, `levelLoad = [wait(3000), showDialog(objective), updateState(ui.textEntry, "")]`. Each machine shows the dialog and clears its own text entry, as the same steps do before the wait.
- [ ] On a client, a UI press firing `[playSound(click), wait(300), returnToFrontend()]`:
  - plays the click;
  - drops the return;
  - warns once, naming the reaction and step;
  - warns no further on a second press.
  
  A crossing over shared state with the same shape warns nothing above debug.
- [ ] Unloading the level, suspending or hot-reloading on a client drops its parked tails, and nothing lands into the next level.
- [ ] Grep and diff gate: `WIRE_VERSION` is unchanged and no `ServerPresentationPayload` variant is added.

**Multi-effect bodies**
- [ ] `defineReaction(name, [a, [b, wait(5)], c])` and `defineReaction(name, { sequence: [a, b, ...wait(5), c] })` emit byte-identical wire data, in TS and Luau alike (research S9).
- [ ] A tracer returning an array and one returning `{ sequence }` both install. Each array form fires as its `{ sequence }` equivalent: by name, by trigger before any wait, and after a wait.
- [ ] Already-flat content emits byte-identical manifest data before and after the change.
- [ ] An empty array body and an empty Luau table body each install as an inert reaction, with no warning above debug.
- [ ] Each of these in an array throws at author time in both runtimes, naming `defineReaction`, and TS rejects each at compile time:
  - `progress`;
  - a nested `{ sequence }`;
  - a reaction handle, whose error suggests `fire`;
  - an `onComplete` primitive.

**Surface**
- [ ] The Scripting surface example ships as the rewritten `ghost-links.{ts,luau}`, on the existing maps. Its TS and Luau twins produce byte-identical wire data. The ghost-links surface tests pass with the merged bodies. In loopback the klaxon plays once on each machine, and no client logs the refused-enrollment warning.

### Manual
- [ ] Host plus client playtest of ghost-links:
  - the plate reveals and swells;
  - each machine hears the klaxon once when the north closet opens;
  - the split and the return behave as before.

## Path
- **Parse.** `sequence_steps_from_js` and `sequence_steps_from_lua` gain a `System` arm ahead of the required-`id` fallthrough. `sequence_primitives_are_valid` consults `SystemReactionRegistry` for it.
- **Dispatch.** `dispatch_sequence` takes the system registry. It calls the `dispatch_system_primitive` path for `System` steps inside the caller's `SystemCommandFireContext`.
- **Trigger partition.** `partition_direct_reaction`'s pre-wait presentation arm admits `System` steps; today it warns on any non-Entity/Group step. `bind_sequence_step` binds a `System` `setState` through `bind_command_at`, as `bind_primitive` does.
- **Bindings and validation.**
  - `SystemReactionIrBindings::rebuild` walks sequence steps.
  - V4a and V4b (`validate_reaction_bodies_pass_a`, `scoped_fire_targets`) read system steps. Retire their "`playSound` is system-only" comments.
  - `player_events::validate`'s `direct_violation` uses the shared predicate over sequence steps.
- **Landing origin.**
  - The scheduler stores each instance's `FireOrigin`, beside or in place of the `InstanceKey` origin pair. `drain_landings` installs that origin, re-resolving seats, instead of `HostOnly(FireSubject::None)`.
  - Trap: `presentation_capture::after_apply` decides override capture from that same `origin` field, so restoring `EveryMachine` would silently stop AC10 capture. Keep capture host-only for host landings separately from routing.
- **Client tails.** `ReactionScheduler` gains a client role that:
  - enrolls only tails holding a machine-local step;
  - lands only those steps;
  - is advanced by the connected-client tick.
  
  Rival: forward host tails over E16's lane. That is right for one-player audiences and wrong for own-state crossings (`research.md`).
- **SDK.**
  - System builders in `sdk/lib/ui/reactions.{ts,luau}` and `postretro/audio` return a `SystemReactionStep` that is assignable to both a body and a step.
  - `ReactionBody` gains nested arrays.
  - `defineReaction` in `sdk/lib/data_script/reactions.ts` and in `data_script.luau` flattens and lowers arrays before computing the auto id.
  - `runtime_sequence_step_shape_is_valid` in `light_membership.rs` accepts system steps.
- **Docs.** The build updates `docs/ghost-links.md` (its "spread into a reaction's `sequence`" and "a whole reaction body" text) and `docs/scripting-reference.md`. Every command there must stay bundle-runnable. `player-events.ts` and `closet-reveal.{ts,luau}` drop their single-effect wrappers.
- **First slice.** Loopback `levelLoad = [wait(500), playSound(x)]`, heard once on each machine. It falsifies the client tick seam and the restored landing origin together.
- **Split first.** These files are past ~800 lines: `reaction_scheduler.rs`, `reaction_validation.rs`, `system_reactions.rs`, `light_membership.rs`, `data_script.ts`, `data_script.luau` and `main.rs`. Split only the ones extended, behavior-preserving, each in its own commit.

## Open questions
- Whether client tails reuse `ReactionScheduler` with a role, or a sibling machine-local-only scheduler — **delegated**. `ui-timed-reactions` argues for one scheduler whose instances carry their own clock.
- Whether ghost-links' new klaxon beat is acceptable churn in a shipped demo — owner — **does not block build**.

## Boundary inventory
Both runtimes ship every row.
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| System step | `SequenceTarget::System` (payload-free) | sequence entry `{ primitive, args }` with no `id`, `kind`, `target`, `tag` or `onComplete`; `args.audience` as for bodies | any system builder, unspread, typed `SystemReactionStep` (`postretro`, `postretro/ui`, `postretro/audio`) | the same builders (`UI.playSound(…)`, `Audio.setGhostAudio(…)`) | n/a |
| Step type | n/a | n/a | `SystemReactionStep` joins `SequenceStep`; system builders return it | Luau step type mirror | n/a |
| Array body | n/a (lowered by the SDK) | `{ sequence: [...] }`, flattened | `ReactionBody` gains nested `ReactionEntry[]`; plain and tracer forms | table body or tracer return, nested tables flattened | n/a |
| Machine-local predicate | one public function over `SystemReactionClass` and slot replication | n/a | n/a | n/a | n/a |
| Client machine-local tail | scheduler client role; per-instance `FireOrigin` | n/a (net wire unchanged) | n/a | n/a | n/a |

## Wire format
Manifest JSON only; the net wire and `WIRE_VERSION` do not change.
- A **sequence entry** gains a system form: `{ primitive, args }` with no `id`, `kind` or `target`. Parse rejects it, naming the reaction, if `primitive` is not a system reaction or if the entry carries `tag` or `onComplete`.
- **Array bodies** never reach the wire; the SDK emits `{ sequence }`.
