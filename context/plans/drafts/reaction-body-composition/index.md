# reaction-body-composition

Brief · resumable · reads: `context/lib/scripting.md` §10.4, §12 · `context/lib/networking.md` §Presentation events vs. replicated state · read at 7ca6092dc

## Problem
The owner requested this capability. It surfaced while building the ghost-links demo (`content/dev/scripts/ghost-links.{ts,luau}`). A reaction cannot bundle different kinds of effect. A system reaction (`playSound`, `setGhostAudio`, `updateState`, `loadLevel`, …) is never a `sequence` step, so `echo.swell` wraps one `setGhostAudio` call that the trigger fires beside `echo.reveal`. A body holds one effect or a `{ sequence }`, so every reaction carries `{ sequence: [...spreads] }`, and the Luau twin hand-rolls flatten helpers. A beat that waits cannot share a body with what fires at once, so the split crossing and the return each fire two reactions. A `levelLoad` tail is refused on every client with a warning. When done, any system reaction is a sequence step; an array body, plain or returned from the `(on) => …` tracer, means a sequence; and after a `wait`, a machine-local step lands on the same machines it would have landed on before the wait.

## Decisions
- **Every system reaction is a legal step.** A step does what the same reaction does as a body, on the machine that runs it. A step with no `id`, `kind` or `target` is a system step; parse rejects one whose primitive is not a system reaction, or that carries `tag` or `onComplete`, naming the reaction and the step, in both runtimes. A closed subset would be a second list to learn, since §10.4 treats all system reactions as one namespace.
- **Wire: a system step is its body descriptor `{ primitive, args }`**, following the group and subject-token precedent (`plans/done/sdk-addressing-model`).
- **A step lands when the same body would land.** Frame-end system steps apply in authored order among themselves. A post-wait step evaluates its reads at its landing.
- **Machine-local is one shared predicate:** never replicated and not consequential — the `Presentation` and `MachineLocal` classes, cell writes included, and `setState` on a slot that does not replicate. The client tail filter, `ui-timed-reactions` and E16's install rule all use it; E16 rejects what is machine-local and not `Presentation`, because it forwards presentation to the event player. `HostConsequence` verbs, `setGhostAudio` and sentiment among them, are never machine-local; clients see them by replication.
- **A wait changes when a machine-local step lands, never where.** Every other step after a wait stays host-only (E18 Invariants). This widens E18's named client-tail follow-up from presentation to machine-local steps and reverses its client-refusal default (owner ruling).
  - A landing routes under the origin its instance parked with, so a host landing of an every-machine tail presents locally and no sound plays twice. A subject whose player has left reaches no machine.
  - The instance key includes the origin class, so a trigger's `fire(r)` and a crossing's fire of `r` park separate instances. Within one class, keying is unchanged.
  - On the host, every member light, fog or emitter step a landing applies, including through its `fire(r)` follow-ups, records as a host-only override: capture follows "the host is landing a tail", not the restored origin, so `plans/done/coop--networked-by-default` AC10 stands.
  - Sources every machine runs and the host mirrors (`levelLoad`, crossings over shared state) park tails on connected clients too. A client lands only their machine-local steps and logs nothing above debug.
  - Own-state and local-only sources run each machine on its own clock and land only machine-local steps on a client, which warns once per reaction naming it and the dropped step (E18's O43): a UI press, a crossing over the machine's own owner-private or local slots, and movement events, which fire only for the machine's own pawn. The executor classifies weapon events by the same test, mirrored or own-pawn.
  - On a client, a post-wait `fire(r)` dispatches `r` through the same filter, an interruptible wait lands as non-interruptible and logs nothing above debug, and landings due in one frame run in enrollment order, as on the host.
  - **Clock.** A client `levelLoad` tail lands at the current level's host `script_time` 0 plus the wait, on the host timeline that replicated animations rebase to (`networking.md` §Presentation state), never on the previous level's clock. One already due at the client's first host-clock reading drops at debug. Every other client tail keeps the client's clock. After a host stall past the tick clamp, a client `levelLoad` tail can land early by the clamped amount; the brief accepts that drift and leaves the host scheduler alone (`research.md` §Clock).
- **The fire context reaches system steps before a wait and not after.** A pre-wait step inherits the fire's context: `at: on.emitter` resolves, dispatch inputs bind, and `audience` and the origin route as on a body. After a wait, install drops a reaction whose system step — its own or a `fire` target's — reads `at: on.emitter`, a dispatch input or any `audience`, literal or token. E16's `players().on` rule walks steps and rejects a non-forwardable machine-local step, as for the same body.
- **E18's cancel and teardown cover system steps.** An Exit cancel and an accepted re-fire restart cover system steps. Teardown, suspend and hot reload drop client tails as they drop host ones.
- **Array bodies are SDK sugar for `{ sequence }`.** The plain body and the tracer's return each take an array or a `{ sequence }`. Arrays may hold `wait` and `fire`. Nested arrays flatten at any depth, in arrays and `sequence` lists alike. An empty array, Luau's empty table included, is an inert empty sequence. A non-step entry — `progress`, a nested `{ sequence }`, a reaction handle, `onComplete` — throws at author time naming `defineReaction`, and TS rejects it at compile time. Rivals: `research.md` §Array bodies.
- **Placement.** Arrays lower in the SDK, so wire, parser, partition, scheduler and light pass see only sequences, and the net wire does not change. Client tails need a client scheduling role; its shape is delegated (Open questions).
- **Lib amendments at promotion.** `scripting.md` §10.4 (system reactions as steps; `setGhostAudio` as "a target-less reaction body"); §12 Role check ("Steps after a `wait` run on the host only, because the scheduler is disabled on clients"); §12 Presentation audience, for presentation builders now being steps; `networking.md` §Presentation state (`wait` landings in the host-only list, and "every `wait` tail runs on the host alone").
- **Non-goals.**
  - Waits in UI-fired reactions (`ui-timed-reactions`); subject tokens after a wait, where E18's rules stand (`research.md` §Doors); multi-effect impact policies, which have their own `Effect` vocabulary.
  - Deprecating `fire(r)`. It stays the idiom for beats shared by several reactions, brush KVPs or a TS/Luau pair; only single-effect wrappers become redundant (`research.md` §`fire(r)` after this brief). E16's `fire` rule and plain-read rejection are unchanged.

### Scripting surface
The ghost-links demo, rewritten (excerpt):
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
  const northCloset = defineReaction("closet.a1", [
    getMapEntities("mover", { tag: "echo_closet_a1" }).map((door) => door.start()),
    npcs({ tag: "echo_monster_a1" }).update({ aggro: true }),
  ]);
  // The host opens the closet; each connected machine sounds the klaxon once, on the host's timeline.
  const northTimer = defineReaction("levelLoad", [
    wait(15000),
    fire(northCloset),                 // reuse: barrier_approach fires it too
    playSound("sfx/test_tone"),
  ]);
  // The crossing into copy B: one body where two reactions used to fire.
  const splitLanded = defineReaction("split.landed", [
    getMapEntities("mover", { tag: "echo_closet_a2" }).map((door) => door.start()),
    npcs({ tag: "echo_monster_a2" }).update({ aggro: true }),
    wait(3000),
    getMapEntities("mover", { tag: "echo_closet_b" }).map((door) => door.start()),
    npcs({ tag: "echo_monster_b" }).update({ aggro: true }),
  ]);
  // … the other reactions take the same shapes. Wiring is unchanged except that echo_plate
  // fires [reveal] and split_to_b fires [splitLanded]; every other trigger and teleport binding stays.
}
```
`return.settle` and `wave.start` merge the same way. Luau nests tables in place of `append` and the flattening half of `each`; mapping over members still needs a loop: `Postretro.defineReaction("closet.a1", { doorSteps, Postretro.npcs({ tag = "echo_monster_a1" }):update({ aggro = true }) })`, where `doorSteps` is a table of step tables.

## Acceptance
Pins are `research.md` §Ordering pins.
### Automated
**System steps**
- [ ] Every kind in `SystemReactionKind::ALL` installs as a step in both runtimes, with the same effect as the same body, fired by name, from `levelLoad`, by crossing, by trigger and by teleport.
- [ ] Parse rejects a targetless step naming `applyDamage`, naming the reaction and step, in both runtimes; also a step carrying `tag` or `onComplete`. The same `playSound` step parses.
- [ ] S1: in one frame the mover starts during the walk, `a` then `b` are written with `b` reading 1, and `x` plays. S2: `alarm` is written in the firing tick, `x` plays at that frame's drain and `y` at landing.
- [ ] S3: `b` reads 2 at landing. An IR-valued `setState` step never logs "not bound at level install"; one targeting a per-owner slot is rejected at install as a body is.
- [ ] S4, S5, S6: the pre-wait forms install and behave as pinned; each post-wait or `fire`-reached form is dropped at install, naming the reaction.
- [ ] S7: neither tail step runs. An accepted re-fire restarts the body and replays its pre-wait system steps once. S8: the cancelled tail writes nothing; the re-entry writes `alarm` and plays `x` once, and its own tail writes `s` once.
- [ ] A sequence holding a light member step and a system step reserves the light's build-time membership exactly as without the system step.
- [ ] `players().on` with `[on.player.damage(5), showDialog(d)]` is rejected at install, naming it; `[on.player.damage(5), playSound(x)]` installs and plays `x` on the event player's machine.
- [ ] Drift guard, by exhaustive match over `SystemReactionKind::ALL` plus `setState` on a replicated and a non-replicated slot: the client tail filter keeps exactly the shared predicate's kinds, and the `players().on` rule rejects exactly those minus `Presentation`.

**Where tails land** (loopback host plus client unless stated)
- [ ] S9: the client plays `x` and dispatches neither other step locally; the mover and NPC change reach it by replication. The host applies all three. Neither logs above debug about the tail.
- [ ] S10: each machine hears `x` exactly once in both forms; the host sends the client no Presentation command for `x`, and neither logs above debug about the tail.
- [ ] S11: `l`'s change reaches the client by replication, inline in the tail and through `fire(r)` alike, and `x` plays once per machine, each from its own landing, with no Presentation command for `x` on the wire.
- [ ] S12: each machine plays `x` twice, as it does with `x` before the wait.
- [ ] S13: each machine plays `x` only for its own crossing, and the host sends the client nothing.
- [ ] S14 (trigger) and S15 (teleport): the flash presents on the client only and `y` plays on every machine once, matching the steps before the wait.
- [ ] S16: with A connected the flash presents on A only; after A disconnects mid-wait it presents on no machine, the host included. `[wait(300), fire(flash)]` in the same reaction is still dropped at install.
- [ ] S17: each machine lands A before B and ends with `d` shown.
- [ ] S18: the slow loader plays `x` from its own landing at host `script_time` 5 s within one tick, the host sending it no Presentation command; a first clock reading one tick before the landing plays `x` once, no later than one tick past it; one tick after, or a joiner after the landing, never plays `x` and logs nothing above debug.
- [ ] S19: the client plays `x` once, at the new level's host `script_time` 5 s within one tick, never at a time read from the previous level.
- [ ] S20 (conditioned loopback): the client plays `x` 500 ms after its own crossing, within one tick. With the second edge inside the host's wait and outside the client's, the host plays `x` once and the client twice, and neither logs above debug. With `r = [wait(800, { interruptible: true }), playSound(x)]` also bound to a trigger's Enter, the client's crossing tail plays `x` after 800 ms and logs nothing above debug.
- [ ] In a trigger-bound `[wait(300), flashScreen(…, { audience })]`, install drops the reaction, naming it, for `audience: "everyone"` and for `on.activators` alike. Before the wait, the same step routes to its audience as the same body does.
- [ ] On a client, a post-wait `setState` on a replicated slot, `setGhostAudio` or `loadLevel` does not run, and a post-wait `fire(r)` lands only `r`'s machine-local steps. On the host all run, and the client sees the slot and the ghost bus by replication. With `levelLoad = [wait(3000), showDialog(objective), updateState(ui.textEntry, "")]`, each machine shows the dialog and clears its own text entry.
- [ ] On a client, a UI press firing `[playSound(click), wait(300), returnToFrontend()]` plays the click, drops the return and warns once, naming the reaction and step; a second press warns no further. A client movement event firing the same shape warns the same way; a crossing over shared state with it warns nothing above debug.
- [ ] With a `levelLoad` tail parked on the client (asserted pending), unloading the level, suspending or hot-reloading drops it, and nothing lands into the next level.
- [ ] Grep and diff gate: `WIRE_VERSION` is unchanged and no `ServerPresentationPayload` variant is added.

**Multi-effect bodies**
- [ ] S21: byte-identical wire data in TS and Luau, for nesting in an array body and in a `sequence` list. Already-flat content emits byte-identical manifest data before and after the change.
- [ ] A tracer returning an array and one returning `{ sequence }` both install; each array form fires as its `{ sequence }` equivalent by name, by trigger before any wait, and after a wait. An empty array body and an empty Luau table body each install as an inert reaction, with no warning above debug.
- [ ] `progress`, a nested `{ sequence }`, a reaction handle (whose error suggests `fire`) and an `onComplete` primitive in an array each throw at author time in both runtimes, naming `defineReaction`, and each fails TS compilation.
- [ ] The Scripting surface example ships as the rewritten `ghost-links.{ts,luau}` on the existing maps; its twins produce byte-identical wire data, the surface tests pass with the merged bodies, and in loopback the klaxon plays once per machine with no refused-enrollment warning.

### Manual
- [ ] Host plus client ghost-links playtest: the plate reveals and swells; every machine connected at 15 s hears the klaxon once, with the host, and a later joiner never does; the split and the return behave as before.

## Path
- **Parse and dispatch.** `sequence_steps_from_js` and `sequence_steps_from_lua` gain a `System` arm ahead of the required-`id` fallthrough, yielding a payload-free `SequenceTarget::System` (no sentinel); `sequence_primitives_are_valid` consults `SystemReactionRegistry` for it. `dispatch_sequence` takes the system registry and runs `dispatch_system_primitive` for `System` steps inside the caller's `SystemCommandFireContext`. `partition_direct_reaction`'s pre-wait presentation arm admits them (today it warns on non-Entity/Group steps), and `bind_sequence_step` binds a `System` `setState` through `bind_command_at`, as `bind_primitive` does.
- **Bindings and validation.** `SystemReactionIrBindings::rebuild` walks steps. V4a (`validate_reaction_bodies_pass_a`) and V4b (`scoped_fire_targets`) read system steps, V4a's audience test widens from `audience_token` to any `audience`, and their "`playSound` is system-only" comments retire. The shared predicate replaces the private `machine_local_effect`, which excludes `Presentation`; `direct_violation` composes it with "not `Presentation`". `descriptor_has_machine_local_effect` is a second, name-based classifier to fold in.
- **Landing origin.** The scheduler stores each instance's `FireOrigin` and keys by origin class; `drain_landings` installs the stored origin, re-resolving the seat from the key's `PlayerId`, instead of `HostOnly(FireSubject::None)`. Trap: `presentation_capture::after_apply` decides capture from that same origin, and the tail's follow-ups dispatch through `dispatch_deferred_named_events_with_sequences` after the previous context is restored. Give capture a host-landing flag spanning both.
- **Client tails.** The client scheduling role, whatever its shape, enrolls only tails holding a machine-local step, lands only those, is advanced by the connected-client tick, and orders same-frame landings by enrollment id. A `levelLoad` tail reads host time through `ClientReplication::presentation_host_time` and waits for that clock; a previous-level snapshot can re-anchor it after unload (`research.md` §Clock). Rival: forward host tails over E16's lane, right for one-player audiences and wrong for own-state crossings (`research.md`).
- **SDK.** System builders in `sdk/lib/ui/reactions.{ts,luau}` and `postretro/audio` return a `SystemReactionStep` assignable to a body and a step. `ReactionBody` gains nested arrays and excludes the non-step entries. `defineReaction` in `sdk/lib/data_script/reactions.{ts,luau}` flattens and lowers before computing the auto id. `runtime_sequence_step_shape_is_valid` in `script-compiler`'s `light_membership.rs` accepts system steps. Update `docs/ghost-links.md` (its "spread into a reaction's `sequence`" and "a whole reaction body" text) and `docs/scripting-reference.md`, keeping every command bundle-runnable; `player-events.ts` and `closet-reveal.{ts,luau}` drop their single-effect wrappers.
- **First slice.** Loopback `levelLoad = [wait(500), playSound(x)]`, heard once on each machine, falsifies the client tick seam and the restored landing origin together.
- **Split first.** Past ~800 lines: `reaction_scheduler.rs`, `reaction_validation.rs`, `system_reactions.rs`, `script-compiler`'s `light_membership.rs`, `data_script.ts`, `data_script.luau`, `main.rs`. Split only those extended, behavior-preserving, each in its own commit.

## Open questions
- Whether client tails take a role on `ReactionScheduler` or a sibling machine-local-only scheduler — **delegated**. `ui-timed-reactions` argues for one scheduler whose instances carry their own clock.
- Whether ghost-links' new klaxon beat is acceptable churn in a shipped demo — owner — **does not block build**.

## Boundary inventory
Both runtimes ship every row.
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| System step | `SequenceTarget::System` (payload-free) | sequence entry `{ primitive, args }` with no `id`, `kind`, `target`, `tag` or `onComplete`; `args.audience` as for bodies | any system builder, unspread, typed `SystemReactionStep` (`postretro`, `postretro/ui`, `postretro/audio`) | the same builders (`UI.playSound(…)`, `Audio.setGhostAudio(…)`) | n/a |
| Step type | n/a | n/a | `SystemReactionStep` joins `SequenceStep`; system builders return it | Luau step type mirror | n/a |
| Array body | n/a (lowered by the SDK) | `{ sequence: [...] }`, flattened | `ReactionBody` gains nested `SequenceStep` arrays, excluding non-step entries; plain and tracer forms | table body or tracer return, nested tables flattened | n/a |
| Machine-local predicate | one public function over `SystemReactionClass` and slot replication | n/a | n/a | n/a | n/a |
| Client machine-local tail | client scheduling role; per-instance `FireOrigin`; origin class in the instance key | n/a (net wire unchanged) | n/a | n/a | n/a |

## Wire format
Manifest JSON only; the net wire and `WIRE_VERSION` do not change.
- A **sequence entry** gains a system form: `{ primitive, args }` with no `id`, `kind` or `target`. Parse rejects it, naming the reaction, if `primitive` is not a system reaction or if the entry carries `tag` or `onComplete`.
- **Array bodies** never reach the wire; the SDK emits `{ sequence }`.
