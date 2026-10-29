# reaction-body-composition

Brief · resumable · reads: `context/lib/scripting.md` §10.4, §12 · `context/lib/networking.md` §Presentation events vs. replicated state · read at 167d6885d

## Problem
The owner requested this capability. It surfaced while building the ghost-links demo (`content/dev/scripts/ghost-links.{ts,luau}`). A reaction cannot bundle different kinds of effect. A system reaction (`playSound`, `setGhostAudio`, `updateState`, `loadLevel`, …) is never a `sequence` step, so `echo.swell` wraps one `setGhostAudio` call that the trigger fires beside `echo.reveal`. A body holds one effect or a `{ sequence }`, so every reaction carries `{ sequence: [...spreads] }`, and the Luau twin hand-rolls flatten helpers. A beat that waits cannot share a body with what fires at once, so the split crossing and the return each fire two reactions. A `levelLoad` tail is refused on every client with a warning. When done, any system reaction is a sequence step; an array body, plain or returned from the `(on) => …` tracer, means a sequence; and a wait changes when an effect happens, never who sees or hears it. Where a wait cannot keep that, install refuses the reaction, naming it and the step.

## Decisions
- **Every system reaction is a legal step.** A step does what the same reaction does as a body, on the machine that runs it. On the wire a system step is its body descriptor `{ primitive, args }`, following the group and subject-token precedent (`plans/done/sdk-addressing-model`); parse rejects a malformed one in both runtimes, naming the reaction and the step (Wire format). A closed subset would be a second list to learn, since §10.4 treats all system reactions as one namespace.
- **A step lands when the same body would land.** Frame-end system steps apply in authored order among themselves, and a `fire` target's steps apply after every step of the body that fires it. A post-wait step evaluates its reads at its landing.
- **Machine-local is one shared predicate:** never replicated and not consequential — the `Presentation` and `MachineLocal` classes, cell writes included, and `setState` on a slot that does not replicate. Its users are the client tail filter, `ui-timed-reactions`, E16's `players().on` install rule and the hosted-trigger UI warning. The last two take what is machine-local and not `Presentation`, because both deliver presentation to its audience. `HostConsequence` verbs, `setGhostAudio` and sentiment among them, are never machine-local; clients see them by replication.
- **A wait changes when an effect happens, never who sees or hears it.** Every step after a wait that is not machine-local stays host-only (E18 Invariants). This widens E18's named client-tail follow-up from presentation to machine-local steps and reverses its client-refusal default (owner ruling).
  - A landing presents to the audience the same step reaches before the wait, so no sound plays twice. A trigger's `fire(r)` and a crossing's fire of `r` park separate instances; two fires of `r` from one origin class share one.
  - At landing, a step routed to the host's own player presents on the host, and one routed to a player who has left presents on no machine. Other steps land as usual: a sound with the default everyone audience still plays.
  - Host-only sources (trigger, teleport, player event, host drains with no subject) park on the host. Host-mirrored sources (`levelLoad`, crossings reading only shared slots) park on connected clients too, and a client lands only their machine-local steps, logging nothing above debug.
  - Own-state sources run on each machine's own clock: UI presses; movement events, which fire only for the machine's own pawn; and crossings reading any own, owner-private or local slot, whose firing is per-machine. A client lands only their machine-local steps and warns once per reaction, naming it and the dropped step (E18's O43).
  - On the host, a landing from a host-only or host-mirrored source records every member light, fog or emitter step it applies, including through its `fire(r)` follow-ups, as a host-only override (`plans/done/coop--networked-by-default` AC10). An own-state landing applies them host-local and uncaptured, and clients drop theirs: your own state stays on your own screen.
  - On a client, a post-wait `fire(r)` dispatches `r` through the same filter, an interruptible wait lands as non-interruptible and logs nothing above debug, and landings due in one frame run in enrollment order, as on the host.
  - **Clock.** A client `levelLoad` tail counts each segment in whole host ticks from the current level's host `script_time` 0, on the host timeline that replicated animations rebase to (`networking.md` §Presentation state), never on the previous level's clock. A `fire(r)` it reaches counts `r`'s waits on that timeline too. Every other client tail keeps the client's clock. After a host stall past the tick clamp, a host-timeline tail can land early by the clamped amount; the brief accepts that drift and leaves the host scheduler alone (`research.md` §Clock).
  - **Late joiner.** A client whose first host-clock reading is past a segment's due time converges state and does not replay presentation. Due UI-stack verbs, text and cell edits and non-replicated `setState` apply in authored order at that reading; due sounds, flashes, shakes, rumbles and vignettes are skipped, logged at debug; later segments land on schedule. A joiner sees the level as it is now, not its history.
- **The fire context reaches system steps before a wait and not after.** A pre-wait step inherits the fire's context: `at: on.emitter` resolves, dispatch inputs bind, and `audience` and the origin route as on a body. After a wait the subject is gone, so install drops a reaction whose system step — its own or a `fire` target's — reads `at: on.emitter`, a dispatch input or an audience token, naming the reaction and the step. A literal audience (`"everyone"`) names no subject: it survives a wait and routes as the same step does before it. E16's `players().on` rule walks steps and rejects a non-forwardable machine-local step, as for the same body.
- **E18's cancel and teardown cover system steps.** An Exit cancel and an accepted re-fire restart cover system steps. Teardown, suspend and hot reload drop client tails as they drop host ones.
- **Array bodies are SDK sugar for `{ sequence }`.** The plain body and the tracer's return each take an array or a `{ sequence }`. Arrays may hold `wait` and `fire`. Nested arrays flatten at any depth, in arrays and `sequence` lists alike. An empty array, Luau's empty table included, is an inert empty sequence. A non-step entry — `progress`, a nested `{ sequence }`, a reaction handle, `onComplete` — throws at author time naming `defineReaction`, and TS rejects it at compile time. Rivals: `research.md` §Array bodies.
- **Placement.** Arrays lower in the SDK, so wire, parser, partition, scheduler and light pass see only sequences, and the net wire does not change.
- **Ghost-links teaches it.** The demo gains a post-wait klaxon in `northTimer`: teaching material for a system step after a wait, and the co-op landing proof.
- **Non-goals.**
  - UI-clock waits (`uiWait`): `ui-timed-reactions` owns them and adds `uiWait` to the client warn-once when it lands. A level `wait` in a UI-fired reaction follows the own-state rule.
  - Subject tokens after a wait, where E18's rules stand (`research.md` §Doors); multi-effect impact policies, which have their own `Effect` vocabulary.
  - Deprecating `fire(r)`. It stays the idiom for beats shared by several reactions, brush KVPs or a TS/Luau pair; only single-effect wrappers become redundant (`research.md` §`fire(r)` after this brief). E16 still rejects a `players().on` reaction whose `fire` reaches presentation, or that holds a plain per-player read.
- **Promotion.** The lib text to amend is listed in `research.md` §Prior commitments.

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
  // The crossing into copy B: one body for the closets on both sides of the wait.
  const splitLanded = defineReaction("split.landed", [
    getMapEntities("mover", { tag: "echo_closet_a2" }).map((door) => door.start()),
    npcs({ tag: "echo_monster_a2" }).update({ aggro: true }),
    wait(3000),
    getMapEntities("mover", { tag: "echo_closet_b" }).map((door) => door.start()),
    npcs({ tag: "echo_monster_b" }).update({ aggro: true }),
  ]);
  // … return.settle and wave.start merge the same way. echo_plate fires [reveal], split_to_b
  // fires [splitLanded] and echo_return fires the merged return body; every other binding stays.
}
```
Luau mirror: `Postretro.defineReaction("closet.a1", { doorSteps, Postretro.npcs({ tag = "echo_monster_a1" }):update({ aggro = true }) })`, where `doorSteps` is a table of step tables.

## Acceptance
Pins are `research.md` §Ordering pins.
### Automated
**System steps**
- [ ] Every kind in `SystemReactionKind::ALL` installs as a step in both runtimes, with the same effect as the same body, fired by name, from `levelLoad`, by crossing, by trigger and by teleport.
- [ ] Parse rejects a targetless step naming `applyDamage`, naming the reaction and step, in both runtimes; also a step carrying `tag` or `onComplete`. The same `playSound` step parses.
- [ ] S1–S3 and S27 as pinned: order and reads within a body.
- [ ] S4–S8 as pinned: scoped reads across a wait, and cancel and restart.
- [ ] A sequence holding a light member step and a system step reserves the light's build-time membership exactly as without the system step.
- [ ] `players().on` with `[on.player.damage(5), showDialog(d)]` is rejected at install, naming it; `[on.player.damage(5), playSound(x)]` installs and plays `x` on the event player's machine.
- [ ] Drift guard, by exhaustive match over `SystemReactionKind::ALL` plus `setState` on a replicated and a non-replicated slot: the shared predicate holds for exactly the `Presentation` and `MachineLocal` classes and the non-replicated `setState`, never for `HostConsequence`; the client tail filter keeps exactly those; the `players().on` rule rejects, and the hosted-trigger warning flags, exactly those minus `Presentation`.

**Who sees it** (loopback host plus client unless stated)
- [ ] S9–S13 and S28 as pinned: every-machine, mixed-origin and own-state tails.
- [ ] S14–S16 and S22–S24 as pinned: subjects at landing, present and departed.
- [ ] In a trigger-bound reaction, `flashScreen(…, { audience: "everyone" })` presents on every machine once, before a `wait(300)` and after it alike; under `levelLoad`, the post-wait form presents once on each machine, from its own landing. With `on.activators`, the pre-wait step reaches the activator only, and the post-wait form drops the reaction at install, naming it and the step.
- [ ] On a client, a post-wait `setState` on a replicated slot, `setGhostAudio` or `loadLevel` does not run, and a post-wait `fire(r)` lands only `r`'s machine-local steps. On the host all run, and the client sees the slot and the ghost bus by replication. With `levelLoad = [wait(3000), showDialog(objective), updateState(ui.textEntry, "")]`, each machine shows the dialog and clears its own text entry.
- [ ] On a client, a UI press firing `[playSound(click), wait(300), returnToFrontend()]` plays the click, drops the return and warns once, naming the reaction and step; a second press warns no further. A client movement event firing the same shape warns the same way; a crossing over shared state with it warns nothing above debug.
- [ ] On a client, a weapon `activate` reaction `[wait(300), playSound(x), updateState(s, 1)]`, `s` replicated: the client's own shot warns once, naming the reaction and `setState`; another player's shot, seen through its observer cue, logs nothing above debug. Both play `x` on the client.

**When it lands**
- [ ] S17, S20 and S25 as pinned: tick counting, same-frame order and the client's own clock.
- [ ] S18, S19, S26, S29 and S30 as pinned: the host timeline, late joiners included.
- [ ] With a `levelLoad` tail parked on the client (asserted pending), unloading the level, suspending or hot-reloading drops it, and nothing lands into the next level.
- [ ] Grep and diff gate: `WIRE_VERSION` is unchanged and no `ServerPresentationPayload` variant is added.

**Multi-effect bodies**
- [ ] S21 as pinned, for nesting in an array body and in a `sequence` list. Already-flat content emits byte-identical manifest data before and after the change.
- [ ] A tracer returning an array and one returning `{ sequence }` both install; each array form fires as its `{ sequence }` equivalent by name, by trigger before any wait, and after a wait. An empty array body and an empty Luau table body each install as an inert reaction, with no warning above debug.
- [ ] `progress`, a nested `{ sequence }`, a reaction handle (whose error suggests `fire`) and an `onComplete` primitive in an array each throw at author time in both runtimes, naming `defineReaction`, and each fails TS compilation.
- [ ] The Scripting surface example ships as the rewritten `ghost-links.{ts,luau}` on the existing maps; its twins produce byte-identical wire data, the surface tests pass with the merged bodies, and in loopback the klaxon plays once per machine with no refused-enrollment warning.

### Manual
- [ ] Host plus client ghost-links playtest: the plate reveals and swells; every machine connected at 15 s hears the klaxon once, with the host, and a later joiner never does; the split and the return behave as before.

## Path
- **Parse and dispatch.** `sequence_steps_from_js` and `sequence_steps_from_lua` gain a `System` arm ahead of the required-`id` fallthrough, yielding a payload-free `SequenceTarget::System` (no sentinel); `sequence_primitives_are_valid` consults `SystemReactionRegistry` for it. `dispatch_sequence` takes the system registry and runs `dispatch_system_primitive` for `System` steps inside the caller's `SystemCommandFireContext`. `partition_direct_reaction`'s pre-wait presentation arm admits them (it warns on non-Entity/Group steps), and `bind_sequence_step` binds a `System` `setState` through `bind_command_at`, as `bind_primitive` does.
- **Bindings and validation.** `SystemReactionIrBindings::rebuild` walks steps. V4a (`validate_reaction_bodies_pass_a`) and V4b (`scoped_fire_targets`) read system steps; V4a's audience test stays on `audience_token`, and their "`playSound` is system-only" comments retire. The shared predicate replaces the private `machine_local_effect`, which excludes `Presentation`; `direct_violation` and `descriptor_has_machine_local_effect` compose it with "not `Presentation`".
- **Landing origin.** The scheduler stores each instance's `FireOrigin` and source class, and keys by origin class; `drain_landings` installs the stored origin instead of `HostOnly(FireSubject::None)`, re-resolving the seat from the key's `PlayerId`. `seat_for_pawn` is `None` for the host's pawn and a departed one alike; `Registry::local_player_pawn` tells them apart, as the teleport drain's departed-seat skip does for residuals. Trap: `presentation_capture::after_apply` decides capture from that same origin, and the tail's follow-ups dispatch through `dispatch_deferred_named_events_with_sequences` after the previous context is restored. Give capture a host-landing flag spanning both, set for host-only and host-mirrored instances only.
- **Client tails.** The client scheduling role, whatever its shape, enrolls a tail whose remaining segments hold a machine-local step or a `fire`, lands only machine-local steps, is advanced by the connected-client tick, and orders same-frame landings by enrollment id. A `levelLoad` tail reads host time through `ClientReplication::presentation_host_time`, counts host ticks per segment and waits for that clock; a previous-level snapshot can re-anchor it after unload (`research.md` §Clock). Rival: forward host tails over E16's lane, right for one-player audiences and wrong for own-state crossings (`research.md`).
- **SDK.** System builders in `sdk/lib/ui/reactions.{ts,luau}` and `postretro/audio` return a `SystemReactionStep` assignable to a body and a step. `ReactionBody` gains nested arrays and excludes the non-step entries. `defineReaction` in `sdk/lib/data_script/reactions.{ts,luau}` flattens and lowers before computing the auto id. `runtime_sequence_step_shape_is_valid` in `script-compiler`'s `light_membership.rs` accepts system steps. Update `docs/ghost-links.md` (its "spread into a reaction's `sequence`" and "a whole reaction body" text) and `docs/scripting-reference.md`, keeping every command bundle-runnable; `player-events.ts` and `closet-reveal.{ts,luau}` drop their single-effect wrappers. The Luau twin's `append` and flattening `each` give way to nested tables; mapping over members still needs a loop.
- **First slice.** Loopback `levelLoad = [wait(500), playSound(x)]`, heard once on each machine, falsifies the client tick seam and the restored landing origin together.
- **Split first.** Past ~800 lines: `reaction_scheduler.rs`, `reaction_validation.rs`, `system_reactions.rs`, `script-compiler`'s `light_membership.rs`, `data_script.ts`, `data_script.luau`, `main.rs`. Split only those extended, behavior-preserving, each in its own commit.

## Open questions
- Whether client tails take a role on `ReactionScheduler` or a sibling machine-local-only scheduler — **delegated**. `ui-timed-reactions` argues for one scheduler whose instances carry their own clock.
- How each weapon-event drain classifies, host-mirrored or own-state, by the test in Decisions, beyond the two cases Acceptance pins — **delegated**.

## Boundary inventory
Both runtimes ship every row.
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| System step | `SequenceTarget::System` (payload-free) | sequence entry `{ primitive, args }` with no `id`, `kind`, `target`, `tag` or `onComplete`; `args.audience` as for bodies | any system builder, unspread, typed `SystemReactionStep` (`postretro`, `postretro/ui`, `postretro/audio`) | the same builders (`UI.playSound(…)`, `Audio.setGhostAudio(…)`) | n/a |
| Step type | n/a | n/a | `SystemReactionStep` joins `SequenceStep`; system builders return it | Luau step type mirror | n/a |
| Array body | n/a (lowered by the SDK) | `{ sequence: [...] }`, flattened | `ReactionBody` gains nested `SequenceStep` arrays, excluding non-step entries; plain and tracer forms | table body or tracer return, nested tables flattened | n/a |
| Machine-local predicate | one public function over `SystemReactionClass` and slot replication | n/a | n/a | n/a | n/a |
| Client machine-local tail | client scheduling role; per-instance `FireOrigin` and source class; origin class in the instance key | n/a (net wire unchanged) | n/a | n/a | n/a |

## Wire format
Manifest JSON only; the net wire and `WIRE_VERSION` do not change.
- A **sequence entry** gains a system form: `{ primitive, args }` with no `id`, `kind` or `target`. Parse rejects it, naming the reaction and the step, if `primitive` is not a system reaction or if the entry carries `tag` or `onComplete`.
- **Array bodies** never reach the wire; the SDK emits `{ sequence }`.
