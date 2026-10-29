# reaction-body-composition

Brief · resumable · reads: `context/lib/scripting.md` §10.4, §12 · `context/lib/networking.md` §Presentation events vs. replicated state · read at 71eb28bf3

## Problem
The owner requested this capability. It surfaced while building the ghost-links demo (`content/dev/scripts/ghost-links.{ts,luau}`). A reaction cannot bundle different kinds of effect. A system reaction (`playSound`, `setGhostAudio`, `updateState`, `loadLevel`, …) is never a `sequence` step, so `echo.swell` wraps one `setGhostAudio` call that the trigger fires beside `echo.reveal`. A body holds one effect or a `{ sequence }`, so every reaction carries `{ sequence: [...spreads] }`, and the Luau twin hand-rolls flatten helpers. A beat that waits cannot share a body with what fires at once, so the teleports into copy B and back each fire two reactions. A `levelLoad` tail is refused on every client with a warning. When done, any system reaction is a sequence step; an array body, plain or returned from the `(on) => …` tracer, means a sequence; and a wait changes when an effect happens, never who sees or hears it. Where a wait cannot keep that, install refuses the reaction.

## Decisions
- **Every system reaction is a legal step.** A step does what the same reaction does as a body, on the machine that runs it. On the wire a system step is its body descriptor `{ primitive, args }`, following the group and subject-token precedent (`plans/done/sdk-addressing-model`); parse rejects a malformed one in both runtimes (Wire format). A closed subset would be a second list to learn, since §10.4 treats all system reactions as one namespace.
- **A step lands when the same body would land.** System steps queued for the frame-end system drain, the batch that applies queued system commands after the frame's event drains, apply there in authored order among themselves. A `fire` target's steps apply after every step of the body that fires it. A post-wait step evaluates its reads at its landing.
- **Machine-local is one shared predicate:** never replicated and not consequential. It holds for the `Presentation` and `MachineLocal` classes, cell writes included, for `setState` on a slot that does not replicate, and on a client for `returnToFrontend`, which leaves only that client's session. The rest of `HostConsequence` (`loadLevel`, `restartLevel`, sentiment, `setGhostAudio`, and `returnToFrontend` on the host) is never machine-local.
- **A wait changes when an effect happens, never who sees or hears it.** Every step after a wait that is not machine-local stays host-only (E18 Invariants). This widens E18's named client-tail follow-up from presentation to machine-local steps and reverses its client-refusal default.
  - A landing presents to the audience the same step reaches before the wait.
  - At landing, a step routed to the host's own player presents on the host, and one routed to a player who has left presents on no machine. Other steps land as usual: a sound with the default everyone audience still plays.
  - Each source has a class. A host-mirrored or own-state tail parks on every machine its source fires on. A client lands only its machine-local steps, dispatches a post-wait `fire(r)` through the same filter, lands an interruptible wait as non-interruptible, and runs landings due in one frame in enrollment order, as the host does.

    | Class | Sources | Clock |
    |---|---|---|
    | Host-only: parks on the host | trigger, teleport, player event, mover, NPC, death, kill-progress threshold, and any other host drain with no subject | the host's |
    | Host-mirrored: the host decides, each machine fires | `levelLoad`; crossings reading only shared slots; weapon events, NPC shots and observer cues included, since the host fires `activate` for every shot | `levelLoad`: the host timeline; the rest: each machine's own |
    | Own-state: each machine fires for its own player | crossings reading any own, owner-private or local slot; movement events, which fire only for the machine's own pawn; UI presses | each machine's own |
  - A `fire` target takes the class and clock of the body that fires it, before a wait or after one: `levelLoad = [fire(intro)]` counts `intro`'s waits on the host timeline. Instances of one reaction are separate per class and clock, so one source's fire never absorbs another's beat; fires under one class and clock share an instance, under E18's re-fire rule.
  - On the host, a landing from a host-only or host-mirrored tail records every member light, fog or emitter step it applies as a host-only override (`plans/done/coop--networked-by-default` AC10). An own-state landing captures a networked target's override as the same step does before the wait; on a client-local target it applies host-local and uncaptured, and clients drop theirs: your own state stays on your own screen.
  - **Clock.** A host-timeline tail counts from the current level's host `script_time` 0, on the timeline replicated animations rebase to (`networking.md` §Presentation state), never on the previous level's clock, and lands on the host's landing tick. After a host stall past the tick clamp it can land early by the clamped amount; the brief accepts that drift and leaves the host scheduler alone (`research.md` §Clock).
  - **Late joiner.** A client whose first host-clock reading is past a segment's due time converges state and does not replay presentation. Due UI-stack verbs, text and cell edits and non-replicated `setState` apply in authored order at that reading; due sounds, flashes, shakes, rumbles and vignettes are skipped, logged at debug; later segments land on schedule. A joiner still runs its own install's pre-wait `levelLoad` segment; only presentation already due in the tail is skipped.
- **The fire context reaches system steps before a wait.** A pre-wait step inherits the fire's context: `at: on.emitter` resolves, dispatch inputs bind, and `audience` and the origin route as on a body. A wait removes subject tokens, `on.emitter` and dispatch inputs, and keeps the origin, which routes the landing. Install drops a reaction whose system step after a wait, its own or a `fire` target's, reads `at: on.emitter`, a dispatch input or an audience token. A literal audience (`"everyone"`) names no subject: it survives a wait and routes as the same step does before it. E16's `players().on` rule walks steps and rejects a non-forwardable machine-local step, as for the same body.
- **E18's cancel and teardown cover system steps.** An Exit cancel and an accepted re-fire restart cover system steps. Teardown, suspend and hot reload drop client tails as they drop host ones.
- **Refusals and diagnostics.** Every parse rejection, install drop and client warn-once names the reaction and the step. A client warns only when an own-state landing drops a step: once per reaction, naming the first dropped step in authored order, with a latch that resets at level change, as E18's O43 does. Every other client tail path logs nothing above debug.
- **Array bodies are SDK sugar for `{ sequence }`.** The plain body and the tracer's return each take an array or a `{ sequence }`. Arrays may hold `wait` and `fire`. Nested arrays flatten at any depth, in arrays and `sequence` lists alike. An empty array, Luau's empty table included, is an inert empty sequence. A non-step entry — `progress`, a nested `{ sequence }`, a reaction handle, `onComplete` — throws at author time naming `defineReaction`, and TS rejects it at compile time. Arrays lower in the SDK, so wire, parser, partition, scheduler and light pass see only sequences. Rivals: `research.md` §Array bodies.
- **Ghost-links teaches it.** The demo gains a post-wait klaxon in `northTimer`: teaching material for a system step after a wait, and the co-op landing proof.
- **Non-goals.**
  - UI-clock waits (`uiWait`): `ui-timed-reactions` owns them and adds `uiWait` to the client warn-once when it lands. A level `wait` in a UI-fired reaction follows the own-state rule.
  - Subject tokens after a wait, where E18's rules stand (`research.md` §Doors); multi-effect impact policies, which have their own `Effect` vocabulary.
  - Deprecating `fire(r)`. It stays the idiom for beats shared by several reactions, brush KVPs or a TS/Luau pair; only single-effect wrappers become redundant (`research.md` §`fire(r)` after this brief).

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
  // The teleport into copy B: one body for the closets on both sides of the wait.
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
- [ ] S1–S8 and S27 as pinned: order and reads within a body, scoped reads across a wait, and cancel and restart.
- [ ] A sequence holding a light member step and a system step reserves the light's build-time membership exactly as without the system step.
- [ ] `players().on` with `[on.player.damage(5), showDialog(d)]` is rejected at install, naming it; `[on.player.damage(5), playSound(x)]` installs and plays `x` on the event player's machine.
- [ ] Drift guard, by exhaustive match over `SystemReactionKind::ALL` plus `setState` on a replicated and a non-replicated slot, on host and client: the shared predicate holds for exactly the `Presentation` and `MachineLocal` classes, the non-replicated `setState` and, on a client only, `returnToFrontend`; the client tail filter keeps exactly those; the `players().on` rule rejects exactly those minus `Presentation`.

**Who sees it** (loopback host plus client unless stated)
- [ ] S9–S16, S22–S24, S28, S31, S33, S36–S38 and S40 as pinned: host-mirrored, mixed-origin and own-state tails, networked and client-local capture, mixed-read crossings, instances shared and inherited, a client's reads at its own landing, a client tail with nothing to land, and subjects at landing, present and departed.
- [ ] In a trigger-bound reaction, `flashScreen(…, { audience: "everyone" })` presents on every machine once, before a `wait(300)` and after it alike; under `levelLoad`, the post-wait form presents once on each machine, from its own landing. With `on.activators`, the pre-wait step reaches the activator only, and the post-wait form drops the reaction at install, naming it and the step.
- [ ] On a client, a post-wait `setState` on a replicated slot, `setGhostAudio` or `loadLevel` does not run, and a post-wait `fire(r)` lands only `r`'s machine-local steps. On the host all run, and the client sees the slot and the ghost bus by replication. With `levelLoad = [wait(3000), showDialog(objective), updateState(ui.textEntry, "")]`, each machine shows the dialog and clears its own text entry.
- [ ] On a client, a UI press firing `[playSound(click), wait(300), loadLevel(m)]` plays the click, drops the load and warns once, naming the reaction and step; a second press warns no further. A crossing over shared state firing it warns nothing above debug. With `returnToFrontend()` in place of the load, the press returns that client to the frontend 300 ms later, and the host stays in the level.
- [ ] On a client, a movement event firing `[wait(300), m.start(), updateState(s, 1)]`, `s` replicated, warns once, naming the reaction and `m.start`, not `setState`; further events warn no further until a level change, after which the first warns again.
- [ ] On a client, a weapon `activate` reaction `[wait(300), playSound(x), updateState(s, 1)]`, `s` replicated: for the client's own shot and for another player's shot, seen through its observer cue, the client plays `x`, drops the write and logs nothing above debug; the host lands the write for both.

**When it lands**
- [ ] S17, S20, S25 and S35 as pinned: tick counting, same-frame order, a re-park before the `fire` child it enrolled with, and the client's own clock.
- [ ] S18, S19, S26, S29, S30, S32, S34 and S39 as pinned: the host timeline, clocks a `fire` target inherits, and late joiners, a due `fire` included.
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
- **Bindings and validation.** `SystemReactionIrBindings::rebuild` walks steps. V4a (`validate_reaction_bodies_pass_a`) and V4b (`scoped_fire_targets`) read system steps; V4a's audience test stays on `audience_token`, and their "`playSound` is system-only" comments retire. The shared predicate replaces the private `machine_local_effect`, which excludes `Presentation`, and takes the machine's role for `returnToFrontend`. Its users are the client tail filter, `ui-timed-reactions` and E16's install rule; `direct_violation` composes it with "not `Presentation`", because E16 forwards presentation to the event player. The hosted-trigger UI warning (`descriptor_has_machine_local_effect`) keeps its own check, which already walks steps.
- **Landing origin.** The scheduler stores each instance's `FireOrigin`, source class and clock, and keys by class and clock; a `fire`-seeded child, which enrolls sourceless today, takes its parent's. `drain_landings` installs the stored origin instead of `HostOnly(FireSubject::None)`, re-resolving the seat from the key's `PlayerId`. The host's pawn is bound to `Seat(0)` at install, so `seat_for_pawn` tells it apart from a departed pawn; `Registry::local_player_pawn` is the fallback only for harnesses with no seat table. A departed `Remote` subject is a seat-table miss; the teleport drain's departed-seat skip is the precedent. Trap: `presentation_capture::after_apply` decides capture from that same origin, and the tail's follow-ups dispatch through `dispatch_deferred_named_events_with_sequences` after the previous context is restored. Give capture a host-landing flag spanning both, set for host-only and host-mirrored instances only.
- **Client tails.** The client scheduling role, whatever its shape, enrolls a tail whose remaining segments hold a machine-local step or a `fire`, lands only machine-local steps, is advanced by the connected-client tick, and orders same-frame landings by enrollment id. A host-timeline tail reads host time through `ClientReplication::presentation_host_time`, counts host ticks per segment and waits for that clock; a previous-level snapshot can re-anchor it after unload (`research.md` §Clock). Rival: forward host tails over E16's lane, right for one-player audiences and wrong for own-state crossings (`research.md`).
- **SDK.** System builders in `sdk/lib/ui/reactions.{ts,luau}` and `postretro/audio` return a `SystemReactionStep` assignable to a body and a step. `ReactionBody` gains nested arrays and excludes the non-step entries. `defineReaction` in `sdk/lib/data_script/reactions.{ts,luau}` flattens and lowers before computing the auto id. `runtime_sequence_step_shape_is_valid` in `script-compiler`'s `light_membership.rs` accepts system steps. Update `docs/ghost-links.md` (its "spread into a reaction's `sequence`" and "a whole reaction body" text) and `docs/scripting-reference.md`, keeping every command bundle-runnable; `player-events.ts` and `closet-reveal.{ts,luau}` drop their single-effect wrappers. The Luau twin's `append` and flattening `each` give way to nested tables; mapping over members still needs a loop.
- **First slice.** Loopback `levelLoad = [wait(500), playSound(x)]`, heard once on each machine, falsifies the client tick seam and the restored landing origin together.
- **Split first.** Past ~800 lines: `reaction_scheduler.rs`, `reaction_validation.rs`, `system_reactions.rs`, `script-compiler`'s `light_membership.rs`, `data_script.ts`, `data_script.luau`, `main.rs`. Split only those extended, behavior-preserving, each in its own commit.

## Open questions
- Whether client tails take a role on `ReactionScheduler` or a sibling machine-local-only scheduler — **delegated**. `ui-timed-reactions` argues for one scheduler whose instances carry their own clock.

## Boundary inventory
Both runtimes ship every row.
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| System step | `SequenceTarget::System` (payload-free) | sequence entry `{ primitive, args }` with no `id`, `kind`, `target`, `tag` or `onComplete`; `args.audience` as for bodies | any system builder, unspread, typed `SystemReactionStep` (`postretro`, `postretro/ui`, `postretro/audio`) | the same builders (`UI.playSound(…)`, `Audio.setGhostAudio(…)`) | n/a |
| Step type | n/a | n/a | `SystemReactionStep` joins `SequenceStep`; system builders return it | Luau step type mirror | n/a |
| Array body | n/a (lowered by the SDK) | `{ sequence: [...] }`, flattened | `ReactionBody` gains nested `SequenceStep` arrays, excluding non-step entries; plain and tracer forms | table body or tracer return, nested tables flattened | n/a |
| Machine-local predicate | one public function over `SystemReactionClass`, slot replication and the machine's role | n/a | n/a | n/a | n/a |
| Client machine-local tail | client scheduling role; per-instance `FireOrigin`, source class and clock; class and clock in the instance key | n/a (net wire unchanged) | n/a | n/a | n/a |

## Wire format
Manifest JSON only; the net wire and `WIRE_VERSION` do not change.
- A **sequence entry** gains a system form: `{ primitive, args }` with no `id`, `kind` or `target`. Parse rejects it if `primitive` is not a system reaction or if the entry carries `tag` or `onComplete`.
- **Array bodies** never reach the wire; the SDK emits `{ sequence }`.
