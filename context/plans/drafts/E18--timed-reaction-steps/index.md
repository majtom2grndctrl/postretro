# E18 — Timed / Delayed Reaction Steps

## Goal

Add `wait` and `fire` steps to the reaction sequence vocabulary so authored choreography can space
discrete effects across time — the "alarm cue, beat, doors slam open, enemies pour out" set-piece. A
sequence is split at install into segments; a host-only scheduler counts ticks between them and
resumes the tail. An `interruptible` wait cancels when the trigger that fired it releases. Delivered
with its first consumer: the closet-reveal fixture's plate drives a timed, interruptible reveal.

## Scope

### In scope

- A `wait` control step inside a `sequence` reaction body: `{ durationMs, interruptible }`.
- A `fire` control step dispatching a named reaction by handle or name, so one authored beat can
  release a tag-targeted primitive (the enemy release) alongside setup-id steps (the door).
- **Segmentation at install.** `wait`/`fire` register as inert `SequencedPrimitiveRegistry` handlers so
  `setupLevel` validation admits them; the install pass then **rewrites each segmented reaction's
  `DataRegistry` body in place to segment 0** and moves the tail into a scheduler-owned table. The rewrite —
  not the side table alone — is what makes every name-based dispatch path run seg0 only.
- A host-only **reaction scheduler** advancing parked instances on the authoritative tick, sibling to
  slot accumulators — enroll at a segment boundary, count down in whole ticks, resume, complete or cancel.
  Enrollment has two sites: in-tick (the trigger dispatch closure) and post-loop (the frame-end drain that
  serves `levelLoad`, death events, mover sounds, progress fires, and `fire` steps).
- Cross-class resumed segments partitioned three ways to match the binder's `classify`: **consequential**
  steps apply host-only post-tick via bound commands, **presentation** steps join the frame-end residual
  drain, **lifecycle** steps enqueue level requests.
- Install-time validation with loud, non-fatal diagnostics (the Install validation table below), including
  automatic Exit-edge registration for interruptible waits.
- `interruptible` cancellation from the paired trigger **Exit** edge; re-fire restart/ignore.
- Level-lifecycle integration: instances cleared on unload/restart; two cycle breakers — a per-level cap on
  concurrent instances, and a chain-depth bound on instances enrolled by another instance's `fire`.
- SDK `wait()` and `fire()` builders (TS + Luau), typedefs regenerated, both-runtime parity.
- Consumer: the closet-reveal fixture authored as a timed, interruptible reveal.

### Out of scope

- **Condition-gated waits** and **revert-to-start** on failure. Revert needs an inverse for every applied
  step; sequences are forward-only dispatch (`dispatch_sequence`, `reaction_dispatch.rs`) with no step
  inverse, and `wait(durationMs, opts?: { interruptible?: boolean })` is the only builder — the typedef is
  the vocabulary (`scripting.md` §7), so no predicate field is authorable. Own spec.
- **Sequence-level interruptibility.** No sequence-level flag exists in `ReactionDescriptor::Sequence` or
  `defineReaction`'s body type; the per-step flag is what this spec ships. Named additive follow-up.
- **A cancel verb** (`cancelSequence`-style). Foreclosed by `SequencedPrimitiveRegistry`: an unregistered
  primitive name is dropped by `sequence_primitives_are_valid` at `setupLevel`, and no task registers one.
  The only v1 cancel causes are the paired trigger Exit and a re-fire.
- **Hard-disarm-cancels-pending-exit** (E18-B surface). It cancels a *trigger exit obligation* in
  `TriggerSystem`'s per-`(trigger, player)` bookkeeping — a different owner from this scheduler's
  pending-instance cancel. Foreclosed today: E18-A pins "a mid-stand `disarmTrigger` does not block that
  player's paired exit," and the Exit arm of `run_authoritative_tick_with_dispatch` keys on `paired_enters`,
  not arm state. **Coupling note for E18-B:** landing hard-disarm will silently disable `interruptible`
  cancel for disarmed triggers; that spec inherits this dependency.
- **Ephemeral dispatch-scope params and fire-context targets on resumed steps.** Rejected at install
  (Install validation table), and for TypeScript authors additionally gated at author time: `fire()` takes
  `Reaction<{}>`, so firing a scoped reaction from a delayed step is a type error (`scripting.md` §12,
  "Author-facing types prevent a scoped reaction from being treated as sourceless"). Luau relies on the
  engine gate, per §12.
- **Replicated transient presentation.** A resumed presentation step is host-local, inheriting E18-A's v1
  limit (`plans/done/E18--trigger-event-fanout`, Out of scope: reliable co-op delivery of
  `playSound`/`flashScreen` stings). In co-op the timed set-piece's cue is a host-only experience; only its
  consequential effects reach other players.
- **Pause-freeze of pending waits.** No engine gate freezes the fixed tick — `main.rs` gates only input on
  `ui_captures_gameplay`; the tick loop and `evaluate_slot_accumulators` are ungated. Waits advance during
  pause, matching shipped accumulator behavior. See Owner decisions.
- **Wall-clock delays.** Delays count authoritative ticks. Under a stall the accumulator clamps
  (`MAX_ACCUMULATOR`), so a wait stretches in wall-clock terms — stated, not corrected (O15).

## Direction

**Problem.** The reaction substrate has no temporal decoupling. `SequenceStep` carries no delay and
`dispatch_sequence` runs every step in one synchronous drain (`reaction_dispatch.rs`); `onComplete` chains
synchronously in the same drain, and a `Sequence` reaction cannot chain at all — `fire_named_event_with_sequences`
pushes to `chained` only from `PrimitiveDescriptor.on_complete`. An author cannot space two effects across
time, so the theatrical beat the engine is built for — reveal, pause, consequence — is unbuildable.

**Prior commitments.**
- *Baked over computed* (`index.md` §2). The wait is resolved at install, not walked at runtime. Because the
  install pass **rewrites the `DataRegistry` body to seg0**, every name-based dispatch path —
  `fire_named_event_with_sequences` → `dispatch_sequence`, and the deferred drain that resolves a name via
  `data_registry.reactions` — reads a body that ends at the first wait. Without that rewrite a side table alone
  would change nothing: those paths resolve reactions out of the registry and would still walk the full body.
  The control-step match arms are still amended (Task 1), but mechanically, not as dispatch logic.
- *M14 IR substrate keeps temporal nodes out* (`plans/done/M14--behavior-ir-substrate`, Out of scope:
  "Stateful / temporal nodes… wall-clock… per-tick state arrives as scope *inputs*, never as evaluator-held
  state"). The delay is not an IR node; it lives in the descriptor and the scheduler.
- *E18-A effect-class split* (`plans/done/E18--trigger-event-fanout`): consequential trigger effects execute
  in-tick from install-bound command lists because the sim tick is VM-free. This spec **refines** that: a
  *delayed* consequential effect cannot run in the firing tick, so it applies host-only post-tick via the same
  bound commands. VM-freeness — the property the invariant exists to protect — is preserved; in-tick-ness is
  not, and only for post-wait segments. The nearest structural precedent is `MoverAutoCloseTimers`, a host-only
  per-tick countdown driving a mover on expiry, whose consequence replicates through mover snapshots and which
  "intentionally does not participate in snapshots, digests, or the connected-client simulation" (the
  `auto_close_timers` field doc on `ScriptingCore`, `crates/postretro/src/session/mod.rs`); it differs in
  granularity — `AutoCloseCountdown::remaining_ms` is float-ms, this countdown is whole ticks. The
  impact-effects deferred despawn (`impact_effects.rs`, `after_ms`) is a second precedent.
  `evaluate_slot_accumulators` corroborates the replication leg: host-only, post-`simulate_tick`, reaching
  clients through state-slot replication (`accumulated_shared_global_converges_without_client_side_evaluation`).
- *E18-C reveal composition* (`plans/done/E18--spawner-and-closet-containment`): "The reveal composes at the
  trigger fan-out — an `openDoor` mover reaction and a `releaseCloset` reaction fired together — **not** one
  reaction body (a body is a single primitive; tag-targeted primitives ride the Primitive path, never a
  `sequence`)." The `fire` step honors this: the tag-targeted primitive stays on the Primitive path and is
  *dispatched* by the sequence, never inlined into it. This is also why the enemy release must be delayed
  rather than left on the enter edge — the aggro gate *is* the containment, and opening it early lets enemies
  aggro through a still-closed door.
- *§12 target resolution and handle identity*. Setup-id and fire-time-tag models are non-interchangeable, so
  the door (setup-id) and the enemies (fire-time-tag) cannot share a step list. §12 also pins the handle form
  `fire()` uses: "When the only thing that fires a reaction lives in the same script, reference the const handle
  directly… the const *is* the identity." `onTriggerEvent` already implements exactly this sugar.
- *E18-D host-only rolls, consequences replicate* (`plans/done/E18--trap-pools-seeded-arming`).
- *reaction-sequencing-primitive deferred per-step timing*. This spec resolves it: a `wait` step.

**Alternatives rejected.**
- *Runtime segmentation* — teach `dispatch_sequence` to stop at a wait and return the tail. Rejected: it needs
  `dispatch_sequence` made public across a crate boundary (`scripting-core` cannot name `ReactionScheduler`)
  and leaves every name-based caller double-running post-wait steps unless each is changed. Install
  segmentation costs two passes plus mechanical match arms, and no dispatch *logic* changes.
- *Per-step flat offset* (`{ after: ms }`). Rejected: `interruptible` needs a pending wait to have identity and
  a cursor; cancelling "the wait" is ambiguous when only pre-stamped offsets exist.
- *Timed `onComplete`* (`{ event, afterMs }`). Scatters one authored beat across N named reactions, fighting the
  one-sequence-one-beat reading the consumer wants.
- *Generalize the existing host-only countdown substrate* (`MoverAutoCloseTimers`, impact-effects despawn). A
  shared "count ticks, run a bound action, clear on teardown" substrate could subsume all three. Rejected here:
  the scheduler carries materially more state (multi-segment tails, pre-partitioned class splits, cancel
  identity, re-fire, a cap), so folding it into `MoverAutoCloseTimers` would bloat that timer. Legitimate future
  consolidation spec; this one cites those timers as precedent rather than merging them.
- *Folding hard-disarm-cancels-pending-exit*. Shares the concept of cancellation but almost no implementation —
  it cancels a trigger exit obligation in `TriggerSystem`, never a scheduler entry. Cost would be a v3 bump on
  `TRIGGER_VOLUMES_VERSION` (`crates/level-format/src/trigger_volumes.rs`, whose reader already carries a
  legacy-version branch) for a feature that is not this spec's consumer. Kept separate.

## Install validation

Two passes, because the inputs arrive at different times in `install_world_cpu`
(`crates/postretro/src/startup/lifecycle_world_cpu.rs`):

- **Pass A — segmentation** runs where `slot_accumulator_bindings.rebuild` sits. It needs only the
  `DataRegistry`: split bodies, convert ms to ticks, rewrite each body to seg0, and apply V1 and V6.
- **Pass B — trigger-coupled validation** runs **after** `install_manifest_events`, because
  `build_trigger_bindings` and the manifest-declared `onTriggerEvent` bindings are both constructed after
  Pass A's slot. V2, V3, and V5 read those bindings. Running them in Pass A would drop the consumer's own
  reveal, whose Enter binding is installed by `install_manifest_events`.

Every rejection is a loud, non-fatal diagnostic: `log::error!` (or `warn!` where noted) naming the reaction
and step index, the offending reaction dropped, the level and all other reactions unaffected — matching
`sequence_primitives_are_valid`, which already drops a whole reaction for one bad step. Hot reload inherits
it: a bad edit costs one reaction and a console line, never the session.

| # | Pass | Authored | Response |
|---|---|---|---|
| V1 | A | `durationMs` zero, negative, NaN, non-finite, or overflowing `u32` ticks | `error!`, drop the reaction. Never a silent 1-tick wait (`NaN as u32` → 0 → `.max(1)`) or a `u32::MAX` countdown |
| V2 | B | `interruptible` wait in a reaction Enter-bound to a trigger whose `TriggerVolumeComponent.fire_mode` is `TriggerFireMode::Once` | `error!`, drop the reaction. Cancelling a `once` fire destroys the set-piece permanently — `update_after_fire` sets `latched`, and `evaluate_trigger_activation` rejects `Once if latched` |
| V3 | B | `interruptible` wait in a reaction with no trigger-Enter binding, counting **both** brush-KVP bindings from `build_inner` and manifest bindings from `install_manifest_events` | `error!`, drop the reaction. The flag has no cancel source |
| V4 | A | Post-wait step with a `SequenceTarget::Activators`/`FiredTrigger` sentinel; a bound command reading **any** seeded dispatch input (today `@occupancy` via `TRIGGER_EVENT_INPUTS`, `@rising` via `APP_DRAIN_DISPATCH_INPUTS`); or a `fire` step whose target reaction's bound program declares any dispatch input | `error!`, drop the reaction. No fire context survives a wait. Stated as "any seeded input", not an exhaustive name list, so it does not go stale |
| V5 | B | `interruptible` wait on an Enter-bound reaction whose trigger emits no Exit edge | **derive it** — insert `(trigger, TriggerEventEdge::Exit)` into `bound_edges` via a new `bind_edge_only` method on `TriggerBindingTable`. `bind_event` cannot be reused: it returns early when `commands.is_empty() && steps.is_empty()`, before the `append_binding` call that is the only existing `bound_edges` inserter |
| V6 | A | `fire` step naming a reaction absent from the registry | `warn!`, drop the step, keep the reaction (mirrors the unknown-event `warn!` in `dispatch_deferred_named_events_with_sequences`) |
| V7 | A | Two `Sequence` reactions sharing one address where either contains a wait | `warn!` and segment each independently. Addressing is many-to-one (§12), and `bind_event` already collects a `matched` Vec — so the segmentation table and instance key are keyed by **registry index**, never by name |

V5 is a derivation, not a rejection: without it the Exit arm of `run_authoritative_tick_with_dispatch`
`continue`s when `on_exit.is_empty()` and `bound_edges` lacks the pair, silently consuming the edge.

## Ordering scenarios

Pin the orderings a task agent must handle. The test tasks cite these rows rather than restating them.

| # | Scenario | Ordering / input | Expected outcome |
|---|---|---|---|
| O1 | Basic delay | `levelLoad` sequence `[presA, wait(800), presB]` | presA at fire; presB at fire-tick + `max(1, ceil(800 * 1000 / 16_667))` = 48 ticks |
| O2 | Sub-tick wait | `durationMs` = 5 | 1 tick; resumes on the next scheduler tick, never the fire tick |
| O3 | Interrupt before elapse | interruptible instance; paired Exit at tick k < landing | remaining segments cancelled; consequence never applies |
| O4 | Interrupt on the landing tick | Exit on the tick the segment would resume | cancel wins — Exit applied before countdown advance |
| O5 | Non-interruptible during countdown | Exit while parked, `interruptible:false` | not cancelled; lands on schedule |
| O6 | Re-fire while parked (interruptible) | same instance key re-fires before landing | prior instance cancelled, fresh instance from segment 0 |
| O7 | Re-fire while parked (non-interruptible) | same key re-fires before landing | re-fire ignored; running instance completes |
| O8 | Two instances land same tick | two parked instances resume on one tick | both land, in stable enrollment order |
| O9 | Wait crosses teardown | level unload while parked | instance dropped; no landing |
| O10 | Over the cap | concurrent instances exceed the per-level cap | excess enrollments `warn!` + drop; already-parked unaffected; seg0 has already run and its tail is abandoned |
| O11 | Multi-player plate | two players enter a per-player plate | one instance per key; each player's Exit cancels only their own |
| O12 | Enroll and evaluate share a tick | in-tick enrollment happens inside `simulate_tick_with_presentation_aim`; `evaluate` runs later in the same `for tick_index` iteration | the scheduler skips instances enrolled since the last `evaluate`, once — a 1-tick wait lands on k+1, never k. The marker is pass-scoped, never `tick_index` |
| O13 | Multi-tick frame, short wait | 50ms frame → 3 ticks; `[playSound, wait(17), screenFlash]` fired on tick 0 | the scheduler's owned-steps queue drains immediately after the `pending_trigger_residuals` loop in the same frame, so seg0's `playSound` is always observed before seg1's `screenFlash` |
| O14 | Zero-tick frame | frame with `ticks == 0` | no `evaluate`; countdowns unchanged; no landing, cancel, or cap check |
| O15 | Max-ticks frame after a stall | 2s stall → accumulator clamps to `MAX_ACCUMULATOR` (250ms), ≤14 ticks (`floor(250ms / 16_667us)`) | `evaluate` runs once per tick; a countdown may land within that frame; wall-clock delay is the authored ms **plus** stalled time — lossy under stalls, stated |
| O16 | Two triggers, one reaction, one player | T1 and T2 both bind reaction R; P enters T1 at tick 10, T2 at tick 12 | two instances (key includes trigger); P's Exit from T1 cancels only T1's |
| O17 | Park at a later wait, Exit arrives | `[a, wait(200,i:true), b, wait(200,i:false), c]`; parked at wait1 when Exit lands | the wait currently parked at governs **both** Exit-cancel and re-fire — wait1 is non-interruptible, so no cancel; `c` lands |
| O18 | Re-fire while parked | same body; re-fire while parked at wait0 (interruptible) vs at wait1 (non-interruptible) | parked at wait0: accepted, restart from segment 0. Parked at wait1: ignored, exactly as O7. One rule — the parked-at wait governs — so O7 and O17 never disagree |
| O18b | Restart leaves later-segment effects applied | accepted re-fire while parked at wait0, after an earlier run's `b` already wrote `setState` | restart re-runs segment 0 forward; effects already applied are **not** reverted. Authors write consequential steps idempotent across restart; revert is a separate spec |
| O19 | Re-fire at the cap | cap full; same key re-fires | the same-key cancel is applied at enrollment before the cap test; re-enrollment for an existing key never counts against the cap, so seg0's work is never stranded |
| O20 | Lifecycle step in a resumed segment | `[wait(1000), loadLevel("next")]`; two other instances parked | enqueues a level request; the requesting frame's remaining ticks and residual drain still run; teardown at the next frame's `drive_boot_state_for_redraw` → `unload_level` drops the other instances |
| O21 | Landing consequence vs mover tick | `[wait(500), moverStart]` landing on tick k | command applied on tick k; first observable motion tick k+1 — the same offset as an immediate trigger-fired `moverStart`; tests assert k+1 for pose |
| O22 | Target despawned during the wait | `[wait(500), setAnimationState]` on E; E despawned before landing | the instance lands exactly once; the step on the missing entity is skipped with a warn; remaining steps in the segment still run |
| O23 | Activator dies or disconnects mid-wait | interruptible instance keyed to player P; P's pawn leaves the capsule set | the paired Exit fires and cancels — death or disconnect is a cancel source, identical to walking off |
| O24 | Connected client | client installs a level whose reactions contain waits | zero instances enroll: the tick body's `is_connected_client()` branch `continue`s before the host block; only landed state replicates |
| O25 | Landing order determinism | N instances land on one tick, enrolled across different ticks, players, triggers | evaluation order is insertion-ordered or `BTreeMap`-keyed, never `HashMap` iteration order (matching `SlotAccumulatorBindings`'s explicit determinism comment); identical across runs for all N including 0 |
| O26 | Scheduler vs slot accumulators | a resumed `setState` writes a slot an accumulator's `accumulate` reads, same tick | `evaluate` runs **before** `evaluate_slot_accumulators` (pinned in Task 1), so the accumulator sees a landing on the landing tick — the same offset an in-tick trigger `setState` produces |
| O27 | `fire` step follow-ups | a resumed segment's `fire` step plus the same frame's residual follow-ups | both feed `dispatch_deferred_named_events_with_sequences` and share its 256-hop batch cap; a landing's follow-ups get no independent budget |
| O28 | Self-retriggering wait | R = `[x, wait(17), fire(R)]` bound to plate T; player P enters twice | the self-`fire` is sourceless, key `(idx, None)`; a trigger entry is key `(idx, Some(T,P))`, so concurrency is one per key, not one total. The loop advances once per **frame-end drain**, not once per tick, and terminates at `MAX_REACTION_CHAIN_DEPTH` with a single warning naming the reaction — the concurrency cap alone never catches it |
| O29 | Malformed duration | `durationMs` = 0, -5, NaN, or 1e12 | V1: `error!`, reaction dropped, level continues; never a silent 1-tick wait or a `u32::MAX` countdown |
| O30 | `fire` a scoped reaction from a delayed step | `fire(r)` where `r` reads `on.rising` | TypeScript: compile error (`Reaction<{}>` parameter). Luau: V4 install rejection |
| O31 | Enrollment from the frame-end drain | `levelLoad` / death / `fire`-step fire of `[presA, wait(17), presB]` after the tick loop has exited | no instance advances during the same `evaluate` pass that follows its enrollment; the 1-tick wait lands on the next host tick identically for frames of 1, 3, and 14 ticks |
| O32 | Enrollment-skip is pass-scoped, not index-scoped | two consecutive frames each delivering `ticks == 1` | the scheduler tracks enrollments made since the last `evaluate` and skips them once; `tick_index` is never the marker (it resets per frame, so an index comparison lands a 1-tick wait late whenever `ticks == 1`) |
| O33 | Landing residual identity | a landing's presentation steps plus two trigger residuals in one frame | the scheduler owns its steps as `Vec<PrepartitionedReactionStep>` and never mints a `TriggerResidualHandle`; the drain never resolves scheduler steps through `trigger_bindings.residual()` and never executes another binding's steps |
| O34 | Name-fired segmented reaction | `S = [x, fire(R)]` where `R = [alarm, wait(800), moverStart]`; S fires | `fire_named_event_with_sequences` on `R` runs **seg0 only** — the door does not open in the same frame — because the install pass rewrote `R`'s registry body to seg0 |
| O35 | `setupLevel` validation admits control steps | a reaction containing `@wait`/`@fire` reaches `validate_sequence_primitives`, which runs before reactions land in the `DataRegistry` | the reaction survives; no `names unknown primitive "wait"` error. `wait`/`fire` are registered as inert handlers, and segmentation runs strictly later |
| O36 | Install-pass ordering | `onTriggerEvent({tag}, "enter", [reveal])` — a manifest binding installed by `install_manifest_events`, after `build_trigger_bindings` | V2/V3/V5 run in Pass B, after `install_manifest_events`; the consumer's reveal is not dropped by V3, and V5's `bound_edges` insert survives |
| O37 | `fire` in segment 0 | `R = [fire(sting), wait(800), moverStart]` on a plate Enter | seg0's `fire` lowers to `DeferredEvent` on both the trigger path (`partition_direct_reaction`) and the sourceless path; it is never dropped by the `SequenceTarget::Entity(_)` guard nor classified as an unknown presentation primitive |
| O38 | Teardown clears both tables | level unload with two instances parked | `clear_surface_lifetime_level_state` clears the segmentation table **and** the scheduler's instance map; no instance survives into level B holding a `BoundTriggerCommand` or level-A `EntityId`s |
| O39 | Suspend/resume mid-wait | window suspend while parked; `clear_surface_lifetime_level_state` runs via the suspend path with no level change | instances are dropped with a `warn!` naming the count, so a half-completed beat is distinguishable from a bug |
| O40 | Consequential vs presentation within one landing | `[wait(200), moverStart, screenFlash]` landing on tick 0 of a 14-tick stall frame | `moverStart` applies post-tick 0 (first motion tick 1); `screenFlash` runs at the frame-end drain up to 13 ticks later. Authored intra-segment order across classes is not preserved on a multi-tick frame — accepted, matching the shipped trigger path |
| O41 | Duplicate reaction address | two `defineReaction("levelLoad", { sequence: [...] })` bodies, one containing a wait | V7 warns; each is segmented independently and keyed by registry index, so neither body is lost and re-fire dedup does not conflate them |
| O42 | Co-op cue on the atmosphere channel | host fires the reveal; one connected client | the alarm's `sharedGlobal` write replicates and the client's crossing fires a client-local presentation reaction; a client joining after the write observes the crossing once from its baseline |

## Acceptance criteria

- [ ] A `levelLoad` sequence `[presA, wait(800), presB]` fires presA at level load and presB exactly
  `max(1, ceil(800 * 1000 / 16_667))` = 48 ticks later, in a headless N-tick harness (O1).
- [ ] A 5ms wait resolves to 1 tick and resumes on the next scheduler tick, never the fire tick (O2, O12).
- [ ] An interruptible instance whose paired Exit arrives before the countdown elapses cancels its remaining
  segments (O3); an Exit on the exact landing tick also cancels (O4).
- [ ] `interruptible:false` ignores the paired Exit during countdown; remaining segments land on schedule (O5).
- [ ] Re-firing an interruptible instance while parked cancels and restarts from segment 0; re-firing a
  non-interruptible one is ignored (O6, O7). A re-fire at a full cap still re-enrolls (O19).
- [ ] A delayed consequential step (`moverStart`) applied by the scheduler reaches a connected client through
  state replication with no client-side scheduler evaluation (O24).
- [ ] A pre-first-wait consequential step still executes inside the firing tick: a trigger-fired
  `[moverStart, wait(N), presB]` starts the mover within `simulate_tick`, with no app drain (E18-A invariant).
- [ ] Level unload/restart drops all pending instances; no step lands after unload (O9, O20).
- [ ] Concurrent instances past the per-level cap warn and drop; already-parked instances are unaffected (O10).
- [ ] A self-`fire` loop terminates at `MAX_REACTION_CHAIN_DEPTH` with a single warning naming the reaction; the
  level keeps running and unrelated instances are unaffected (O28).
- [ ] A per-player plate entered by two players yields two instances; each Exit cancels only its own (O11).
- [ ] Two instances landing on one tick both land in stable enrollment order (O8), and two headless runs with
  identical inputs produce identical landing ticks and identical final state (O25).
- [ ] Each Install validation row V1–V4 drops exactly the offending reaction with a diagnostic naming the
  reaction and step index; the level installs and every other reaction is unaffected.
- [ ] V5: an interruptible wait on an Enter-bound reaction causes the install binder to register the trigger's
  Exit edge, and cancellation works with no authored `on_exit` KVP.
- [ ] A `fire` step dispatches its named reaction on the tick its segment lands; `fire()` accepts a reaction
  handle and a string, and a missing target warns without dropping the reaction (V6).
- [ ] `wait` and `fire` do not trip `sequence_primitives_are_valid`; a sequence naming an unknown *action*
  primitive is still rejected as before.
- [ ] The same timed sequence authored in TypeScript and in Luau produces the same landing behavior.
- [ ] The typedef drift check passes with regenerated `sdk/types/postretro.d.ts` / `.d.luau`; snapshot typedef
  tests in `crates/postretro/src/scripting/typedef/tests/` cover the `wait` and `fire` step shapes.
- [ ] A resumed presentation step never precedes an earlier segment's presentation in the same frame, on a
  frame delivering three ticks (O13), and the scheduler never mints a `TriggerResidualHandle` (O33).
- [ ] A reaction containing `wait`/`fire` survives `setupLevel` validation (O35), and firing a segmented
  reaction by name runs segment 0 only (O34).
- [ ] Two sequences sharing one address, either containing a wait, are segmented independently and keyed by
  registry index — neither body is lost and re-fire does not conflate them (O41, V7).
- [ ] V2/V3/V5 run after `install_manifest_events`: the consumer's manifest-bound reveal is not dropped by
  V3, and its derived Exit edge survives (O36).
- [ ] A `fire` step in segment 0 dispatches in the same frame's deferred drain (O37).
- [ ] Level unload clears both the segmentation table and the scheduler's instances (O38); a suspend-path
  clear drops parked instances with a `warn!` naming the count (O39).
- [ ] Parked-at-wait governs both cancel and re-fire: a re-fire while parked at a non-interruptible wait is
  ignored, and one while parked at an interruptible wait restarts from segment 0 without reverting effects
  already applied (O17, O18, O18b).
- [ ] `ReactionScheduler::evaluate` runs before `evaluate_slot_accumulators`, so a landed `setState` is
  visible to that tick's accumulator (O26).
- [ ] No VM invocation occurs on a landing tick, asserted via `crates/test-log-capture` (VM-free invariant).
- [ ] A `sdk/type-tests/` `@ts-expect-error` case proves `fire()` rejects a scoped reaction, alongside the
  existing `invalidScopeErasure` case in `sdk/type-tests/e18-dispatch-params.ts` (O30).
- [ ] Consumer: on the closet-reveal fixture the plate's `enter` raises the alarm immediately; the door does not
  start and the enemies are not released until after the wait; leaving the plate during the wait leaves the door
  closed and the enemies contained; and the plate can be re-entered to run the beat again.
- [ ] The consumer's alarm reaches a connected client: the host's `sharedGlobal` write replicates and the
  client's crossing fires its local light reaction, so both players see the cue (O42).

## Tasks

### Task 1: `wait`/`fire` descriptors, segmentation at install, and the scheduler — thin slice

Add the control steps and the timing seam end to end for a `levelLoad`, presentation-only sequence,
falsifying descriptor → deserialize → segmentation → scheduler → landing tick before any fan-out. Wire shapes
are `{ id: "@wait", primitive: "wait", args: { durationMs, interruptible } }` and `{ id: "@fire", primitive:
"fire", args: { event: string } }` (Boundary inventory). Extend `sequence_steps_from_js`
(`crates/scripting-core/src/data_descriptors/js/reactions.rs`) and its twin `sequence_steps_from_lua`
(`.../lua/reactions.rs`) to accept the `"@wait"` and `"@fire"` sentinels — today every spelling other than
`"@activators"`/`"@trigger"` returns `DescriptorError::InvalidSequenceShape`. Parse `durationMs` and apply
validation row V1 (`error!`, drop the reaction) and `interruptible` (default `false`). Represent control steps
distinctly from entity-targeted actions in `crates/entities/src/data_descriptors/types/reactions.rs`: a
control entry carries no `EntityId` and dispatches to no `SequencedPrimitiveRegistry` handler. Adding a
`SequenceTarget` variant breaks exhaustive matches — enumerate and amend `dispatch_sequence` and
`fire_prepartitioned_reactions_with_sequences` (`crates/scripting-core/src/reaction_dispatch.rs`) plus
`partition_direct_reaction` and `bind_sequence_step` (`crates/postretro/src/trigger_bindings.rs`); a new
variant otherwise falls into `dispatch_sequence`'s "sentinel target has no trigger fire context" warn-arm and
`classify`'s Presentation default. Register `"wait"` and `"fire"` in `SequencedPrimitiveRegistry` as **inert no-op handlers** — otherwise
`sequence_primitives_are_valid` / `validate_sequence_primitives` / `validate_scoped_sequence_primitives`
(`crates/scripting-core/src/reaction_dispatch.rs`) drop the whole reaction at `setupLevel`, before segmentation
ever runs; those functions iterate every step and reject any `primitive` absent from the registry.
**Segmentation runs at install and is what keeps those paths wait-free:**
add a segmentation pass, in a new module (do not grow `trigger_bindings.rs`, already 2590 lines), that walks
every `Sequence` reaction in the `DataRegistry` — not only trigger-bound ones — and splits its body at each
`wait` into segments plus inter-segment whole-tick counts and per-wait `interruptible`. Key the table by
**registry index** (a `ReactionIndex(usize)` newtype over the position in `DataRegistry.reactions`), not by
`NamedReaction.name`: addressing is many-to-one (§12), `bind_event` already collects a `matched` Vec of
same-named reactions, and a name key would silently conflate two `levelLoad` sequences (V7). Then **rewrite
`DataRegistry.reactions[idx].descriptor` in place to segment 0**, stashing the tail in the table — this rewrite,
not the table, is what makes `fire_named_event_with_sequences` and the deferred drain run seg0 only, since both
resolve bodies out of the registry. State that the rewrite happens after `rebuild_reaction_subscribers` and is
re-derived from scratch on every install and hot reload, so it is idempotent. Convert ms to ticks with
integer micros against `TICK_DURATION`; that constant is `pub(crate)` in the `postretro` binary and invisible to
`scripting-core`, so either keep `durationMs` in the descriptor and convert during the install pass (inside
`postretro`) or move the constant to `postretro-foundation` — pick one and state it. Add a `ReactionScheduler`
owned on the session beside `slot_accumulator_bindings` (new file under
`crates/postretro/src/scripting/systems/`), storing instances in a `BTreeMap` or insertion-ordered container
per O25 and the `SlotAccumulatorBindings` determinism precedent. Do **not** stamp instances with a tick index: `tick_index` resets every frame
(`for tick_index in 0..ticks`) and there is no monotonic authoritative counter in single-player — `NetEndpoint::Host { tick }`
advances only when a net endpoint exists. Instead track enrollments made since the last `evaluate` and skip each
once, so an instance never advances during the `evaluate` pass following its enrollment (O12, O31, O32); this is
immune to frame boundaries and to enrollment sites outside the tick loop. Enrollment has **two** sites: in-tick,
which requires `sim::TriggerTickContext` (`crates/postretro/src/sim/mod.rs`) to gain a scheduler borrow and its
`main.rs` construction site to be amended; and post-loop, in the frame-end drain that serves `levelLoad`, death
events, `drain_mover_sound_events_with_sequences`, progress fires, and `fire` steps. Guard enrollment itself
host-only at the scheduler entry point, not merely at the tick-loop call site, since the `levelLoad` fire runs
outside `simulate_tick` on clients too (O24). Countdowns are whole-tick integers, so `evaluate` takes no `dt`.
Wire `evaluate` into the host tick loop in `main.rs` **before** `evaluate_slot_accumulators` (O26), and into
`run_headless_inner` (`observability/driver.rs`) and `SimHarness` (`sim/determinism_tests.rs`). Unit tests:
O1, O2, O12, O14, O29, O31, O32, O34, O35, O41.

### Task 2: SDK `wait()` and `fire()` builders and typedefs

Add both builders to the SDK, mirroring the `armTrigger`/`disarmTrigger` precedent in
`sdk/lib/data_script.ts` (each returns a one-element `SequenceStep[]` with a sentinel id).
`wait(durationMs: number, opts?: { interruptible?: boolean }): SequenceStep[]` emits `{ id: "@wait", primitive:
"wait", args: { durationMs, interruptible: opts?.interruptible ?? false } }`. `fire(reaction: Reaction<{}> |
string): SequenceStep[]` emits `{ id: "@fire", primitive: "fire", args: { event } }`, resolving the name
exactly as `onTriggerEvent` already does — `typeof reaction === "string" ? reaction : reaction.name` — so a
handle works whether its id was authored or derived by `autoReactionId`. Typing the parameter `Reaction<{}>`
(not `Reaction<S>`) makes firing a scoped reaction from a delayed step a compile-time error via the
`reactionScopeBrand` phantom, which is §12's author-facing gate; Luau relies on the V4 engine gate. Author both
Luau twins in `sdk/lib/data_script.luau`. Add `WaitStep` and `FireStep` to the `SequenceStep` union in
`sdk/lib/data_script.ts` and to the generated templates
(`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` and `.luau`), **and** add `wait`/`fire` entries to
the per-function export table in `crates/scripting-core/src/typedef/templates/virtual_module.luau` — without
them a new global is unreachable from a Luau mod. Regenerate typedefs and update the committed/snapshot tests
in `crates/postretro/src/scripting/typedef/tests/`. Add a `@ts-expect-error` negative case to `sdk/type-tests/`, beside the shipped `invalidScopeErasure` case in
`sdk/type-tests/e18-dispatch-params.ts`, proving `fire()` rejects a scoped reaction (O30); that file's
`tsconfig.json` sets `strict: true`, which is what makes the contravariant brand bite. Both-runtime parity
fixture: the same timed sequence in TS and Luau. Depends on Task 1 only for the settled wire shapes in the Boundary inventory.

### Task 3: Cross-class resumed segment execution

Execute a resumed segment's steps, partitioned three ways to match the binder's `classify`
(`crates/postretro/src/trigger_bindings.rs`), which returns `PrimitiveClass::{Consequential, Lifecycle,
Presentation}` — the existing `ReactionDescriptor::Sequence` arm of `partition_direct_reaction` tests only
`== Consequential`, so lifecycle silently falls into the presentation residual and this task fixes that for
resumed segments. At install, pre-partition each post-wait segment into bound consequential commands
(`BoundTriggerCommand`, `crates/postretro/src/trigger_commands.rs`), a presentation residual
(`PrepartitionedReactionStep`), and lifecycle requests; lower each `fire` step to
`PrepartitionedReactionStep::DeferredEvent(event)` in **every** segment including segment 0, on both the
trigger-bound path (`partition_direct_reaction`) and the sourceless path — otherwise a seg0 `fire` is dropped by
that arm's `matches!(step.id, SequenceTarget::Entity(_))` guard with a misleading sentinel-target warning (O37). The partition
*classifier* is reused but the *binding* is not identical to the trigger path: `partition_direct_reaction` is
reached only from `TriggerBindingTable::bind_event`, keyed `(EntityId, TriggerEventEdge)`, so a
non-trigger-origin reaction is new binding work keyed to a reaction; `classify`, `CONSEQUENTIAL_PRIMITIVES`,
`bind_sequence_step`, and `bind_command` are private to `trigger_bindings.rs` and need visibility changes.
Executing a `BoundTriggerCommand` is not free-standing — `execute` requires `&MoverCommandDiagnostics`,
`&SpawnContext`, and `&TriggerFireContext`, and `execute_with_script_ctx` additionally a `&mut DispatchScope`
(for `BoundStoreValue::Ir`); today those are private fields of `TriggerBindingTable` seeded in
`build_with_script_ctx_and_diagnostics`. Give the scheduler a `SchedulerTickContext` mirroring
`sim::TriggerTickContext` that carries exactly the borrows the two apply paths need. `BoundTriggerCommand::execute` takes
`(&mut EntityRegistry, &mut SlotTable, &MoverCommandDiagnostics, &SpawnContext, &TriggerFireContext)`;
`execute_with_script_ctx` takes `(&mut EntityRegistry, &ScriptCtx, &mut DispatchScope,
&MoverCommandDiagnostics, &SpawnContext, &TriggerFireContext)` — note it substitutes `ScriptCtx` for the slot
table, so the context must carry both. `dispatch_scope` is a `RefCell<DispatchScope>` seeded in `build_inner`
with `TRIGGER_EVENT_INPUTS`, not in `build_with_script_ctx_and_diagnostics`, so the `&mut` comes from a borrow.
V4 guarantees no resumed step needs a live fire context, so a `TriggerFireContext::default()` suffices.
This replaces a bare `evaluate(&mut self)` signature. It carries **no** primitive registries:
presentation drains later in `main.rs`, so a landing makes no VM call, which is what the VM-free invariant asserts; V4 guarantees no resumed step needs a live fire context, so a default `TriggerFireContext` suffices
and must be stated as such. On expiry, apply consequential commands host-only post-tick by calling
`BoundTriggerCommand::execute` / `execute_with_script_ctx` directly rather than re-deriving a core per variant —
there are 11 variants and only the mover core (`apply_mover_command_to_targets`,
`crates/postretro/src/kinematic_mover/commands.rs`) would otherwise be named,
hand the presentation steps to a **scheduler-owned** `Vec<PrepartitionedReactionStep>` queue drained in
`main.rs` immediately after the `pending_trigger_residuals` loop and feeding the same `pending_trigger_follow_ups`
(O13, O33). Do not append to `pending_trigger_residuals`: it is a `Vec<TriggerResidualHandle>` resolved through
`trigger_bindings.residual(handle)`, a bare index into `TriggerBindingTable::residuals`, so a scheduler entry
would either fail to resolve or silently execute another binding's steps. Both `PrepartitionedReactionStep` and
`fire_prepartitioned_reactions_with_sequences` are `pub`, so the scheduler owns its steps directly and mints no
handle. Enqueue lifecycle requests (O20). Segment 0's execution keeps its
in-tick bound-command path, but the binder's sequence arm changes to stop at the first wait: it currently binds
every consequential step regardless of position, and post-wait steps must no longer appear in the trigger
binding. Register **two** clears — the segmentation table's and the `ReactionScheduler`'s instance map; that function
enumerates its clears field by field, so an unregistered structure survives a level change. Note it is also the
**suspend** path, so a suspend mid-wait drops parked instances (O39) — emit a `warn!` naming the count. Register
them in `clear_surface_lifetime_level_state`
(`crates/postretro/src/startup/lifecycle_net.rs`) and `rebuild(...)` in `install_world_cpu`
(`crates/postretro/src/startup/lifecycle_world_cpu.rs`, which reaches the binder via `build_trigger_bindings` in
`startup/lifecycle.rs`), at the same point as `slot_accumulator_bindings.rebuild` — Pass A only; Task 4's Pass B
runs later, after `install_manifest_events`. `WorldInstallHandles` (`startup/lifecycle.rs`) gains a field, so amend its **three** construction sites in that
file and the destructure in `install_world_cpu` (`startup/lifecycle_world_cpu.rs`). Enforce a per-level cap on concurrent instances (`MAX_PENDING_REACTION_INSTANCES = 256`, a named const in the
scheduler module; warn + drop past it, and note seg0 has already run and its tail is abandoned). Size it as a
cycle breaker, not a content budget: four players across thirty timed plates is 120 legitimate concurrent
instances, so a smaller cap would bite authored co-op content while a runaway grows without bound either way.
Carry a chain-depth counter alongside it: an instance enrolled by a `fire` step in another instance's landing
inherits depth + 1, and past `MAX_REACTION_CHAIN_DEPTH = 256` the enrollment warns once and drops. Depth bounds
causal chains — self-loops and mutual recursion — where the concurrency cap cannot, because a one-at-a-time loop
never raises the instance count. Both mirror `MAX_BATCH_DISPATCH_HOPS` in
`crates/scripting-core/src/reaction_dispatch.rs`: bound the runaway, warn, keep the level running. Unit tests: O8, O9, O10,
O13, O20, O21, O22, O26, O27, plus a delayed `moverStart` landing host-only.

### Task 4: Install validation and Exit-edge derivation

Implement Pass B (V2, V3, V5) as a post-pass over `trigger_bindings` **after** `install_manifest_events`
in `install_world_cpu` — not beside `slot_accumulator_bindings.rebuild`, which runs before
`build_trigger_bindings` exists, so a V3 check there would drop the consumer's own manifest-bound reveal (O36).
V1, V4, V6, and V7 belong to Pass A alongside segmentation (Task 1 owns V1's parse-site check). V2: reject an `interruptible` wait whose reaction is Enter-bound to a trigger whose
`TriggerVolumeComponent.fire_mode` (`crates/entities/src/components/trigger_volume.rs`) is
`TriggerFireMode::Once` — read the component from the `EntityRegistry`, not the level-format
`TriggerVolumeRecord.fire_mode`, which is a `u8` and out of scope at binding time. The latch is spent on first
fire (`update_after_fire` sets `trigger.latched = true`; `evaluate_trigger_activation` rejects `Once if latched`,
both in `crates/postretro/src/trigger_system.rs`), so a cancel would end the set-piece permanently.
V3: reject an `interruptible` wait in a reaction with no trigger-Enter binding, counting bindings from both
`build_inner` (brush KVPs) and `install_manifest_events` (`onTriggerEvent`) — the flag has no cancel source.
A reaction that is both Enter-bound and a `fire` target still enrolls a sourceless instance on the `fire` path;
that instance treats its interruptible waits as non-interruptible and warns once at enrollment.
V4: reject a post-wait step carrying a `SequenceTarget::Activators`/`FiredTrigger` sentinel; a bound command
reading **any** seeded dispatch input — today `@occupancy` (`TRIGGER_EVENT_INPUTS`, `trigger_bindings.rs`) and
`@rising` (`APP_DRAIN_DISPATCH_INPUTS`, `crates/postretro/src/scripting/systems/system_reactions.rs`), stated as
a category so the rule does not go stale; or a `fire` step whose target reaction's bound program declares any
dispatch input, which the `Reaction<{}>` type gate cannot catch through the `| string` overload or from Luau.
`BoundTarget::Activators`/`FiredTrigger` resolve against `TriggerFireContext` (`BoundTarget::resolve`,
`crates/postretro/src/trigger_commands.rs`); none of these survive a wait. V5 is a derivation, not a rejection: when an Enter-bound reaction contains an `interruptible` wait, insert
`(trigger, TriggerEventEdge::Exit)` into `bound_edges` through a **new** `bind_edge_only` method on
`TriggerBindingTable`. `bind_event` cannot be reused — it returns early on
`commands.is_empty() && steps.is_empty()`, before the `append_binding` call that is the only existing inserter,
and `bound_edges` is private with a read-only accessor. The insert makes the Exit arm of
`run_authoritative_tick_with_dispatch` stop `continue`-ing past the edge — without this the edge is
silently consumed by `paired_enters.remove` and no cancel source exists. V6: warn and drop a `fire` step naming
an absent reaction, keeping the rest of the reaction. Every rejection is `log::error!` naming the reaction and
step index, drops exactly that reaction, and leaves the level and other reactions installable — matching
`sequence_primitives_are_valid`'s existing whole-reaction drop, so hot reload degrades to one lost reaction plus
a console line. Unit tests: one per row, plus the V5 end-to-end that cancellation works with no authored
`on_exit` KVP. Depends on Task 1's segmentation.

### Task 5: Interruptible cancellation and re-fire

Cancel parked instances from paired Exit edges and define re-fire. Instance identity is `(ReactionIndex, Option<(trigger, player)>)` — the registry index from Task 1's
segmentation table, never the reaction name, since one address may carry several reactions (V7) — this single key
governs O6/O7 re-fire dedup *and* O11/O16 per-player and per-trigger separation; a reaction bound to two
triggers yields two instances. Surface the tick's Exit fires into `TickEvents`: production discards the
returned `TriggerFireReport` (both dispatch arms in `simulate_tick_with_presentation_aim` bind `let _report =
…`), and the existing `trigger_fires` field is `#[cfg(test)]`, so add a production (non-`cfg(test)`) field fed
from **both** per-edge dispatch closures in `crates/postretro/src/sim/mod.rs`, and mirror it into `RecordedTick` via `SimHarness::record`
(`crates/postretro/src/sim/determinism_tests.rs`) — that file imports the real `TickEvents` and projects it into
`RecordedTick`; there is no clone of the struct. Name the new field and its `main.rs` reader, which must run
before `evaluate`. Identity comes from `TriggerEvent.fire`
(a `TriggerEventFire { trigger, player, event_name }`) filtered by `TriggerEvent.edge == Exit`;
`TriggerFireReport.fires` is `Vec<TriggerEvent>`. In `ReactionScheduler::evaluate`, apply cancels **before**
advancing countdowns so an Exit on the landing tick wins (O4). With multiple waits, the wait currently parked at governs **both** Exit-cancel and re-fire (O17, O18): parked
at an interruptible wait, a re-fire is accepted and restarts from segment 0; parked at a non-interruptible one,
it is ignored exactly as O7. A restart does not revert effects already applied by later segments (O18b). Apply a re-fire's same-key
cancel at enrollment, before the cap test, so a re-enrollment never fails against the cap and never strands
seg0's already-applied effects (O19). Plumb the Exit-fire set into `run_headless_inner` and `SimHarness` too —
without it the cancel scenarios cannot be tested there. Unit tests: O3–O7, O11, O16–O19, O23. Depends on
Task 4 for derived Exit edges and Task 1 for enrollment.

### Task 6: Consumer — closet-reveal timed reveal

Author the closet-reveal fixture as a timed, interruptible reveal. In
`content/dev/maps/closet-reveal.map`: add `"_tags" "closet_alarm"` to the `light_spot`, and change the
`closet_reveal_plate` trigger's `"fire_mode"` from `"once"` to `"multiple"` (the `fire_mode` match in `crates/level-compiler/src/trigger_volumes.rs` maps
`"once" | "0" => 0` and `"multiple" | "1" => 1`, and `bail!`s on any other spelling; the plate's existing
`"rearm_ms" "0"` means `update_after_fire` re-arms immediately, which is what makes re-fire reachable) — required for
interruptibility per V2, and it lets the fixture exercise re-fire (O6/O7). Rewrite
`content/dev/scripts/closet-reveal.ts` so the plate's `enter` fires one sequence: an immediate alarm cue driven
through the **co-op atmosphere channel** — a `setState` on a `sharedGlobal` slot (`encounter.alarm`), which each
client's crossing watcher turns into a client-local `setLightAnimation` on `closet_alarm`. E18-A shipped that
path end-to-end including late-join (`plans/done/E18--trigger-event-fanout`); a host-local
`setLightAnimation` step would leave remote players watching a door open with no cue. A reddening alarm is
persistent state, not a transient sting, so it belongs on the channel built for state rather than the one E18-A
documents as lossy for edges. The light is static, and `LightAnimation.color` is accepted on authored static
lights (`validate_and_normalize`, `crates/lighting/src/script_primitives.rs`), so the client-local reaction may
animate intensity or colour. Declare the slot in the fixture's store. Then
`wait(N, { interruptible: true })`, then `moverStart` on the `closet_door` mover resolved by
`world.query({ component: "kinematic_mover", tag: "closet_door" })`, then `fire(releaseCloset)` dispatching the
existing tag-targeted enemy release. The release must be *dispatched*, not inlined: E18-C pins that
tag-targeted primitives ride the Primitive path and never a `sequence`, and `BoundTriggerCommand::UpdateEnemyState`
requires `BoundTarget::Tag` while `bind_sequence_step` maps a `SequenceTarget::Entity` to `BoundTarget::Entity`.
Keep `releaseCloset` as its own `defineReaction` so `fire()` takes its handle. Delaying it is load-bearing, not
cosmetic: the aggro gate is the containment (E18-C), so releasing at the enter edge would let enemies aggro
through a still-closed door for the whole beat. Fixture check backing the consumer AC: alarm at press, door and
enemies only after the wait, leaving the plate mid-wait leaves the door closed and enemies contained, and the
plate re-fires. Depends on Tasks 2, 3, 4, 5.

### Task 7: Ordering, determinism, and replication coverage

Author the cross-cutting headless tests for Ordering rows not covered by a single task's units — O15 (stall
clamp stretching the wall-clock delay), O24 (connected client enrolls nothing), O25 (landing-order determinism
across two identical runs), O28 (self-retriggering wait accepted), O30 (Luau-side V4 rejection of a scoped `fire`), O39 (suspend-path clear), O40 (intra-landing class split) — plus the co-op replication check that a delayed `moverStart` reaches a client with no client-side
scheduler, using the harness pattern in `netcode/state_slot_loss_harness_test.rs`. Use the N-tick headless
drivers (`run_headless_inner`, `SimHarness`) seeding a fixed `1.0/60.0` DT. Land new test modules as new files
rather than growing `sim/determinism_tests.rs` (3652 lines). Depends on Tasks 3 and 5.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies descriptor → deserialize → install segmentation →
scheduler → landing tick before fan-out.
**Phase 2 (concurrent):** Task 2 (SDK builders — needs only the settled wire shapes), Task 3 (resumed-segment
execution).
**Phase 3 (sequential):** Task 4 — shares `install_world_cpu` (`startup/lifecycle_world_cpu.rs`) with Task 3,
which registers Pass A there; Task 4 adds Pass B after `install_manifest_events`.
**Phase 4 (sequential):** Task 5 — consumes Task 4's derived Exit edges and Task 1's enrollment.
**Phase 5 (concurrent):** Task 6 (consumer), Task 7 (ordering/determinism/replication tests).

## Boundary inventory

| Name | Rust (internal) | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| wait step | control entry in the sequence body (distinct from an action `SequenceStep`) | `{ "id": "@wait", "primitive": "wait", "args": {…} }` | `wait(durationMs, opts)` → `SequenceStep[]` | `wait(durationMs, opts)` | n/a |
| fire step | control entry lowering to `PrepartitionedReactionStep::DeferredEvent` | `{ "id": "@fire", "primitive": "fire", "args": { "event": … } }` | `fire(reaction \| name)` → `SequenceStep[]` | `fire(reaction \| name)` | n/a |
| wait sentinel | control target variant | `"@wait"` | `"@wait"` | `"@wait"` | n/a |
| fire sentinel | control target variant | `"@fire"` | `"@fire"` | `"@fire"` | n/a |
| delay | whole tick count (install-time integer conversion) | `"durationMs"` (number, ms) | `durationMs` | `durationMs` | n/a |
| interruptible | `bool` | `"interruptible"` (bool, default false) | `interruptible` | `interruptible` | n/a |
| fire target | reaction identity string | `"event"` (string) | `Reaction<{}> \| string` | `Reaction \| string` | n/a |

Milliseconds cross the wire; ticks are internal. Conversion is integer micros against `TICK_DURATION`
(`crates/postretro/src/frame_timing.rs`, `Duration::from_micros(16_667)`):
`ticks = max(1, ceil(durationMs * 1000 / 16_667))`. Float `1000/60` is not the constant — it yields 49 where the
integer rule yields 48. Existing sentinels `"@activators"`/`"@trigger"` are the naming precedent; `fire()`
resolves a handle to its name exactly as `onTriggerEvent` does.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| No runtime dispatch path encounters a `wait` — the install pass rewrites each segmented body to seg0 | Task 1 (registry rewrite + inert handlers) | a side table alone changes nothing: name-based paths resolve bodies out of `DataRegistry`; and unregistered control primitives are dropped at `setupLevel` | O34, O35 |
| A resumed segment applies with no VM call — bound commands only; presentation drains later in `main.rs` | Task 3 (pre-partition; `SchedulerTickContext` carries no primitive registries) | carrying a registry into the landing would make this unenforceable by inspection | VM-free AC (log-capture) |
| Delayed consequential effects are host-authoritative and replicated, never client-re-simmed | Task 3 (host-only post-tick apply) | clients run no scheduler; a client-side eval would double-apply | Co-op replication AC, O24 |
| An instance lands exactly once per segment boundary, or is cancelled exactly once — never both | Task 1 (enroll/resume), Task 5 (cancel) | cancel must precede countdown advance in a tick (O4); a re-fire must not leave two instances (O6, O19) | O3, O4, O6, O19 |
| Countdowns advance only on authoritative host ticks, in whole ticks, never during the `evaluate` pass following enrollment | Task 1 (integer countdown, enrolled-since-last-pass set, host-only enroll guard) | a per-frame `tick_index` marker lands a 1-tick wait late whenever `ticks == 1`; a client-side advance breaks replication | O1, O2, O12, O31, O32, O25 |
| Immediate (pre-first-wait) consequential effects still run in-tick | Task 3 (seg0 keeps the in-tick path; binder stops at the first wait) | the binder currently binds every consequential step regardless of position | Pre-wait `moverStart` AC |
| A resumed presentation step never precedes an earlier segment's presentation in the same frame | Task 3 (scheduler-owned steps queue drained right after the residual loop) | appending to `pending_trigger_residuals` cannot work — it holds bare indices into `TriggerBindingTable::residuals` | O13, O33 |
| Level teardown drops all pending instances with no landing | Task 3 (two clears in `clear_surface_lifetime_level_state`) | that function clears field by field, so an unregistered structure survives; it is also the suspend path | O9, O20, O38, O39 |
| An `interruptible` wait always has a live cancel source, or is inert and warned | Task 4 (V2, V3 reject; V5 derives the Exit edge via `bind_edge_only`) | the Exit arm `continue`s when unbound; `bind_event` early-returns before the only existing inserter; a sourceless `fire`-path instance has no Exit at all | V-row ACs, O36, Consumer AC |
| No resumed step reads fire-time context | Task 4 (V4 rejects sentinels, any seeded input, and scoped `fire` targets), Task 2 (`Reaction<{}>` gate) | the binder otherwise pre-partitions `@activators`/`@trigger` targets and `@occupancy`/`@rising` readers into a post-wait segment; the type gate misses the `\| string` overload and all Luau | O30, V4 AC |

## Script syntax examples

```typescript
// Proposed design — closet-reveal timed reveal (Task 6).
import { defineReaction, enemies, fire, onTriggerEvent, setState, wait, world } from "postretro";

// Tag-targeted, so it stays its own reaction on the Primitive path (E18-C).
const releaseCloset = defineReaction(
  "closet.releaseCloset",
  enemies({ tag: "closet_enemies" }).update({ aggro: true }),
);

export function setupLevel() {
  const door = world.query({ component: "kinematic_mover", tag: "closet_door" });

  // One authored beat: raise the alarm now, hold, then slam and release together.
  // Stepping off the plate during the hold cancels both. The alarm rides the
  // sharedGlobal channel so every player sees it, not just the host.
  const reveal = defineReaction("closet.timedReveal", {
    sequence: [
      setState("encounter.alarm", 1),        // sharedGlobal — replicates; clients light it locally
      ...wait(800, { interruptible: true }),  // splits the body here at install
      ...door.flatMap((m) => m.start()),      // consequential, setup-id
      ...fire(releaseCloset),                 // dispatches the tag-targeted release
    ],
  });

  return {
    reactions: [reveal, releaseCloset],
    triggerEvents: [
      // No "exit" registration needed — V5 derives the Exit edge from the interruptible wait.
      onTriggerEvent({ tag: "closet_reveal_plate" }, "enter", [reveal]),
    ],
  };
}
```

## Owner decisions

Two ship-time choices that no source forecloses — surfaced rather than buried.

- **Waits advance while the pause menu is open.** No engine-wide pause concept exists to inherit —
  `renderer.freeze_time()` gates the animation sample clock only. No engine gate freezes the fixed tick: `main.rs` gates only
  input on `ui_captures_gameplay`, while the tick loop and `evaluate_slot_accumulators` run ungated. The
  scheduler matches shipped accumulator behavior. Freezing on pause would be new cross-cutting scope this spec
  does not own. Default chosen: advance. Flag if a set-piece needs pause-freeze.
- **A wall-clock delay stretches under stalls.** `MAX_ACCUMULATOR` clamps the tick accumulator, so a 2s stall
  delivers ≤14 ticks and a `wait(800)` takes 800ms plus the stalled time (O15). Correcting this would mean
  wall-clock delays, which co-op determinism rules out. Default chosen: tick-counted, stretch accepted.
