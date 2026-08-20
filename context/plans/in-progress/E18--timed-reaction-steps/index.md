# E18 — Timed / Delayed Reaction Steps

## Goal

Add `wait` and `fire` steps to the reaction sequence vocabulary so authored choreography can space
discrete effects across time — the "alarm cue, beat, doors slam open, enemies pour out" set-piece. A
`wait` step enrolls the rest of its own body with a host-only scheduler and stops; the scheduler counts
authoritative ticks and resumes the remainder through the shipped residual path. An `interruptible` wait
cancels when the trigger that fired it releases. Delivered with its first consumer: the closet-reveal
fixture's plate drives a timed, interruptible reveal.

## Scope

### In scope

- A `wait` control step inside a `sequence` reaction body: `{ durationMs, interruptible }`.
- A `fire` control step dispatching a named reaction, so one authored beat can release a tag-targeted
  primitive (the enemy release) alongside setup-id steps (the door).
- **Enrollment at dispatch.** `dispatch_sequence` gains a control arm ahead of its
  `SequenceTarget::Entity` guard. On `@wait` it hands the remaining steps, the wait's args, and the
  reaction address to a registered control handler, then `break`s. Every *named* dispatch path funnels
  through `dispatch_sequence`, so enrollment reaches all of them — including hops inside
  `dispatch_deferred_named_events_with_sequences` — without enumerating any of them.
- **A binder amendment, because trigger-bound bodies do not use that funnel.**
  `partition_direct_reaction` pre-partitions a trigger-bound `Sequence` at install and never calls
  `dispatch_sequence` for the consequential portion. Its arm is amended to stop at the first `Wait`: steps
  before the wait partition as today, and the wait plus everything after it goes to the residual in
  authored order. The residual drains at frame end through `dispatch_sequence`, so the wait meets the
  control arm there.
- A host-only **`ReactionScheduler`** advancing parked instances on the authoritative tick, sibling to
  slot accumulators. Its shared handle is `Rc<RefCell<_>>`, modelled on `MoverAutoCloseTimers`, and is
  cloned into the control handlers at registration in `session/mod.rs`.
- **Resume through the shipped residual path.** On expiry the scheduler wraps its stored tail in
  `PrepartitionedReactionStep::Descriptor(ReactionDescriptor::Sequence(tail))` and hands it to
  `fire_prepartitioned_reactions_with_sequences` at the frame-end drain — the same call the
  `pending_trigger_residuals` loop already makes — feeding the returned `chained` names into the existing
  `dispatch_deferred_named_events_with_sequences`.
- Install-time validation with loud, non-fatal diagnostics (the Install validation table below), including
  automatic Exit-edge registration for interruptible waits. The passes are **read-only**: they inspect
  bodies and bindings and reject reactions; they never rewrite a body.
- `interruptible` cancellation from the paired trigger **Exit** edge; re-fire restart/ignore. Cancel
  identity comes from a scoped **origin guard** held across dispatch entry points that know their origin.
- Level-lifecycle integration: instances cleared on unload/restart/suspend; two cycle breakers — a
  per-level cap on concurrent instances, and a chain-depth bound on instances enrolled by another
  instance's `fire`.
- SDK `wait()` and `fire()` builders (TS + Luau), the full export chain, typedefs regenerated, both-runtime
  parity.
- Consumer: the closet-reveal fixture authored as a timed, interruptible reveal.

### Out of scope

- **Condition-gated waits** and **revert-to-start** on failure. Revert needs an inverse for every applied
  step; sequences are forward-only dispatch (`dispatch_sequence`, `reaction_dispatch.rs`) with no step
  inverse, and `wait(durationMs, opts?: { interruptible?: boolean })` is the only builder — the typedef is
  the vocabulary (`scripting.md` §7), so no predicate field is authorable. Own spec.
- **Sequence-level interruptibility.** No sequence-level flag exists in `ReactionDescriptor::Sequence` or
  `defineReaction`'s body type — `Sequence(Vec<SequenceStep>)` and `{ sequence: SequenceStep[] }` cannot
  represent the state. The per-step flag is what this spec ships. Named additive follow-up.
- **A cancel verb** (`cancelSequence`-style). A scope choice, not a foreclosure: registering a primitive is
  a one-line `SequencedPrimitiveRegistry::register` call and this spec registers two. The v1 cancel causes
  are the paired trigger Exit and a re-fire; a verb would need its own identity model for "which instance,"
  which the origin guard supplies only at dispatch time. Own spec.
- **Hard-disarm-cancels-pending-exit** (E18-B surface). It cancels a *trigger exit obligation* in
  `TriggerSystem`'s per-`(trigger, player)` bookkeeping — a different owner from this scheduler's
  pending-instance cancel. Foreclosed today: E18-A pins "a mid-stand `disarmTrigger` does not block that
  player's paired exit," and the Exit arm of `run_authoritative_tick_with_dispatch` keys on `paired_enters`,
  never on `armed` or `latched`. **Coupling note for E18-B:** landing hard-disarm will silently disable
  `interruptible` cancel for disarmed triggers; that spec inherits this dependency.
- **Ephemeral dispatch-scope params and fire-context targets on resumed steps.** Rejected at install
  (V4a/V4b), and for TypeScript authors additionally gated at author time: `fire()` takes `Reaction<{}>`, so
  firing a scoped reaction from a sequence is a type error (`scripting.md` §12, "Author-facing types
  prevent a scoped reaction from being treated as sourceless"). Luau relies on the engine gate.
- **Replicated transient presentation.** A resumed presentation step is host-local, inheriting E18-A's v1
  limit (`plans/done/E18--trigger-event-fanout`, Out of scope: reliable co-op delivery of
  `playSound`/`flashScreen` stings). Consequential effects reach other players through state and mover
  replication. The consumer's cue therefore rides the atmosphere channel rather than a transient sting.
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

**The mechanism.** A reaction body reached *by name* is a `Vec<SequenceStep>` walked only by
`dispatch_sequence`. That funnel is the seam. The `@wait` arm enrolls `&steps[i + 1..]` with the scheduler
and breaks, so the tail is the remainder of the body the dispatcher already holds — nothing is baked into
the body, nothing is keyed by position, and nothing has to be re-derived when the registry is rebuilt.

One path bypasses it, and it is the one the consumer uses. A trigger-bound `Sequence` is pre-partitioned at
install by `partition_direct_reaction`, whose arm binds every step `classify` calls `Consequential` into an
in-tick command **regardless of position**, and drops any non-`Entity`-target step in its presentation
branch with "sentinel target on presentation sequence step … not binding". Left unamended it would delete
the wait and both `fire` steps from the consumer's body and open the door in the firing tick. The arm is
amended to stop at the first `Wait`, which both restores the funnel — the residual reaches
`dispatch_sequence` at the frame-end drain — and is what delivers the E18-A guarantee that a pre-wait
consequential step still runs in-tick.

This matters because `DataRegistry.reactions` is a **derived view**: `recompose_active_sets`
(`crates/entities/src/data_registry.rs`) rebuilds it by cloning from the retained `global_reactions` and
`level_reactions`, and is referenced from thirteen sites across four files — five production paths in
`startup/lifecycle.rs`, the staged-manifest hot-reload commit (`staged_manifest_lifecycle.rs`), three in
`scripting-core/src/runtime/core.rs`, and four in `data_registry.rs` itself, including the definition and
its tests. Any design
that mutates a body at install is undone by a recompose that does not re-run the mutation. Reading the tail
out of the live dispatch is immune to that by construction.

The same funnel property answers "which call sites enroll." `fire_named_event_with_sequences` is reached
from `install_world_cpu` (the `levelLoad` fire), the UI button `NamedReaction` path, `commit_text_entry`,
`dispatch_state_crossings_with_sequences`, the frame-end residual drain, and — critically — from hops
*inside* `dispatch_deferred_named_events_with_sequences`, whose loop lives in `scripting-core` and cannot
name a `postretro` type. Enrollment sited in `dispatch_sequence` covers all of them without any of them
being enumerated.

**Prior commitments.**
- *Baked over computed* (`index.md` §2). Segmentation is not baked because the artifact it would bake into
  is regenerated. What is resolved once at install is *validation* — V1–V6 reject malformed and
  context-dependent bodies before any frame runs, so the runtime arm is a stop-and-enroll with no analysis.
- *M14 IR substrate keeps temporal nodes out* (`plans/done/M14--behavior-ir-substrate`, Out of scope:
  "Stateful / temporal nodes… wall-clock… per-tick state arrives as scope *inputs*, never as evaluator-held
  state"). The delay is not an IR node; it lives in the descriptor and the scheduler.
- *E18-A effect-class split* (`plans/done/E18--trigger-event-fanout`): consequential trigger effects execute
  in-tick from install-bound command lists because the sim tick is VM-free. This spec preserves that
  exactly. `dispatch_sequence` is never called inside `simulate_tick` — in-tick trigger work runs
  `BoundTriggerCommand`s and returns a `TriggerResidualHandle`; all sequence dispatch happens at the
  frame-end drain or from named fires outside the tick loop. A resumed segment therefore runs where trigger
  residuals already run, and no VM call is added to the tick.
- *E18-A precedent for the countdown*: the nearest structural precedent is `MoverAutoCloseTimers`, a
  host-only per-tick countdown driving a mover on expiry, whose consequence replicates through mover
  snapshots and which "intentionally does not participate in snapshots, digests, or the connected-client
  simulation" (the `auto_close_timers` field doc on `ScriptingCore`, `crates/postretro/src/session/mod.rs`).
  Its `Rc<RefCell<_>>` clone-handle, captured into sequenced mover primitives by
  `register_sequenced_mover_primitives`, is the exact shape `ReactionScheduler` takes. It differs in
  granularity — `AutoCloseCountdown::remaining_ms` is float-ms, this countdown is whole ticks. The
  impact-effects deferred despawn (`impact_effects.rs`, `after_ms`) is a second precedent.
  `evaluate_slot_accumulators` corroborates the replication leg: host-only, post-`simulate_tick`, reaching
  clients through state-slot replication (`accumulated_shared_global_converges_without_client_side_evaluation`).
- *E18-C reveal composition* (`plans/done/E18--spawner-and-closet-containment`): "The reveal composes at the
  trigger fan-out — an `openDoor` mover reaction and a `releaseCloset` reaction fired together — **not** one
  reaction body (a body is a single primitive; tag-targeted primitives ride the Primitive path, never a
  `sequence`)." The `fire` step honors this: the tag-targeted primitive stays on the Primitive path and is
  *dispatched* by the sequence, never inlined into it. The same rule governs the consumer's alarm, which is
  a system-targeted `setState` and therefore also a dispatched Primitive reaction rather than a step —
  `bind_sequence_step` refuses a `setState` step outright ("setState is system-targeted and cannot carry an
  entity target; not binding"), and no `register_sequenced_*` call registers it. This is also why the enemy
  release must be delayed rather than left on the enter edge — the aggro gate *is* the containment, and
  opening it early lets enemies aggro through a still-closed door.
- *§12 target resolution and handle identity*. Setup-id and fire-time-tag models are non-interchangeable, so
  the door (setup-id) and the enemies (fire-time-tag) cannot share a step list. §12 also pins the handle form
  `fire()` uses: "When the only thing that fires a reaction lives in the same script, reference the const handle
  directly… the const *is* the identity." `onTriggerEvent` already implements exactly this sugar.
- *E18-D host-only rolls, consequences replicate* (`plans/done/E18--trap-pools-seeded-arming`).
- *reaction-sequencing-primitive deferred per-step timing*. This spec resolves it: a `wait` step.

**Alternatives rejected.**
- *Segmentation at install, rewriting the body to segment 0.* Rejected: `DataRegistry.reactions` is
  rebuilt by `recompose_active_sets` from retained originals, so the rewrite is erased by every hot reload,
  level-tag change, and return-to-frontend unless the pass re-runs at every one of them; and any
  position-derived key retargets when that rebuild reorders the vector (globals filtered by level tag, then
  locals appended). It also leaves the enrollment site unsolved for hops inside
  `dispatch_deferred_named_events_with_sequences`.
- *An enrollment sink threaded through the dispatch signatures.* Rejected: it changes
  `dispatch_sequence`, `fire_named_event_with_sequences`, and
  `dispatch_deferred_named_events_with_sequences` signatures across a crate boundary to solve what a
  captured handle solves with none, and the sink still has to be plumbed to every caller.
- *Per-step flat offset* (`{ after: ms }`). Rejected: `interruptible` needs a pending wait to have identity and
  a cursor; cancelling "the wait" is ambiguous when only pre-stamped offsets exist.
- *Timed `onComplete`* (`{ event, afterMs }`). Scatters one authored beat across N named reactions, fighting the
  one-sequence-one-beat reading the consumer wants.
- *Generalize the existing host-only countdown substrate* (`MoverAutoCloseTimers`, impact-effects despawn). A
  shared "count ticks, run a bound action, clear on teardown" substrate could subsume all three. Rejected here:
  the scheduler carries materially more state (stored tails, cancel identity, re-fire, a cap, a depth
  counter), so folding it into `MoverAutoCloseTimers` would bloat that timer. Legitimate future
  consolidation spec; this one reuses its handle shape rather than merging with it.
- *Folding hard-disarm-cancels-pending-exit*. Shares the concept of cancellation but almost no implementation —
  it cancels a trigger exit obligation in `TriggerSystem`, never a scheduler entry. Cost would be a v3 bump on
  `TRIGGER_VOLUMES_VERSION` (`crates/level-format/src/trigger_volumes.rs`, whose reader already carries a
  legacy-version branch) for a feature that is not this spec's consumer. Kept separate.

## Enrollment, origin, and identity

Three facts fix the whole timing model, and every ordering row below follows from them.

**Enrollment never happens inside the tick loop, but it can happen before one.** `dispatch_sequence` is
never reached from inside `for tick_index in 0..ticks` — in-tick trigger work executes
`BoundTriggerCommand`s and defers its steps as a residual handle. It *is* reached from two different
phases of a frame: **before** the tick loop, at `fire_focused_button_activation` (the
`UiButtonAction::NamedReaction` `on_press` and `commit_text_entry` `on_commit` paths, both reached from
the `RedrawRequested` arm ahead of the loop), and **after** it, at the frame-end residual drain, the
deferred-hop loop, and `run_crossing_stage`. `install_world_cpu`'s `levelLoad` fire precedes every frame.

Being outside the loop is therefore not sufficient to keep an instance from advancing in its own frame: a
pre-loop enrollment would be advanced by that same frame's `evaluate` passes. The scheduler stamps each
instance with a **monotonic frame counter** — incremented once per `RedrawRequested`, never reset, unlike
`tick_index` — and `evaluate` skips any instance whose stamp equals the current frame. That is one
comparison, it is immune to which phase enrolled, and it makes the landing offset identical for every
entry point.

**Provenance is a triple, and it is carried, never re-derived.** Enrollment needs
`(ReactionAddress, BodyOrdinal, Option<(EntityId, PlayerId)>)`. Each component has exactly one origin point
and must be transported from it; every attempt to reconstruct one later is wrong, because the reconstruction
sites lack the information.

*Address and ordinal* originate in the loop that resolves a name to bodies. `fire_named_event_with_sequences`
walks `data_registry.reactions` matching on name, so it knows both the address and the index of this body
among the same-named matches. `dispatch_sequence` takes both as parameters and passes them to the control
handler. The residual path has neither, and the address cannot live on `TriggerResidual`: `bind_event`
partitions **all** of a `(trigger, edge)`'s matched reactions into one `commands`/`steps` pair before
calling `append_binding`, which further *merges* into any existing residual for that pair, so one residual
is N reactions' product. The address and ordinal therefore ride on the **step** —
`PrepartitionedReactionStep::Descriptor` carries them — which is also what supplies the parameter
`fire_prepartitioned_reactions_with_sequences` needs in order to call `dispatch_sequence` at all.

*Origin* originates in the per-edge dispatch closures in `crates/postretro/src/sim/mod.rs`, where
`event.fire.trigger` and `event.fire.player` are in scope. Today only the handle survives them —
`trigger_residuals: Vec<TriggerResidualHandle>`, pushed as `trigger_residuals.push(handle)` — so the
frame-end drain iterates opaque indices and cannot know which player fired what. `TickEvents.trigger_residuals`
becomes `Vec<(TriggerResidualHandle, EntityId, PlayerId)>`, fed from both closures, which already clone
`event` under `#[cfg(test)]` and so demonstrably have it. The drain sets `ReactionScheduler`'s
`current_origin` per iteration from the tuple rather than once across the loop. Without this, two players
entering one plate push the *same* handle twice and collapse to a single instance, and O52's paired-enter
check degenerates to "is anyone standing here" — both failures a bystander on the plate makes visible.

Deferred-batch hops stay explicitly **outside** the guard: one batch mixes follow-up names seeded by
different residuals and its hops chain inside `scripting-core` with no `postretro` code between them, so a
guard held across the batch would attribute one residual's origin to another's reaction. A `fire`-seeded
instance is therefore always sourceless, which is what V3's demotion rule already states.

*A resumed tail carries all three from its own instance.* The landing drain runs outside the origin guard,
and a resumed tail has no `matched` loop to re-derive an ordinal from, so an instance stores its address,
ordinal, and origin at enrollment and the resume passes them back down. Re-deriving instead would make a
nested wait silently sourceless — losing `interruptible` one wait into a multi-wait body — and would give
every same-named body ordinal 0, so the second landing's re-enrollment would cancel the first.

**Instance identity is `(InstanceKey, InstanceId)`.** `InstanceKey` is that triple. `ReactionAddress` is the
reaction *name* — stable across a recompose where positions are not. `BodyOrdinal` keeps two `levelLoad`
sequences from cancelling each other: address and origin alone are identical for both, so a same-key re-fire
cancel would let only the last-dispatched body survive. `InstanceId` is a monotonically increasing `u64`
assigned at enrollment; it is the sole landing-order sort key (O8, O25). Re-fire dedup and Exit cancel both
match on `InstanceKey`.

## Install validation

Two read-only passes, because the inputs arrive at different times in `install_world_cpu`
(`crates/postretro/src/startup/lifecycle_world_cpu.rs`), whose order is `rebuild_reaction_subscribers` →
`slot_accumulator_bindings.rebuild` → `build_trigger_bindings` → `install_manifest_events` → the
`levelLoad` fire:

- **Pass A — body validation** runs where `slot_accumulator_bindings.rebuild` sits. It needs only the
  `DataRegistry`: walk every `Sequence` body, apply V1, V4a, and V6. It inspects; it does not rewrite.
- **Pass B — trigger-coupled validation** (V2, V3, V4b, V5) runs **after** `install_manifest_events`, because
  `build_trigger_bindings` and the manifest-declared `onTriggerEvent` bindings are both constructed after
  Pass A's slot. V2, V3, and V5 need reaction-to-trigger provenance, which `TriggerBindingTable` does not
  record — `bind_event` resolves a name to a `matched` Vec and immediately partitions it into anonymous
  commands and steps, and `bound_edges` is a `HashSet<(EntityId, TriggerEventEdge)>`. Pass B therefore
  reads its provenance from the sources the binder read: `TriggerVolumeComponent.on_fire` / `on_exit`
  across `EntityRegistry` triggers, plus `data_registry.trigger_events` (tag → trigger ids ×
  `descriptor.fire` names). Both passes must also run wherever `recompose_active_sets` is followed by a
  binding rebuild, so a hot reload re-validates rather than inheriting a stale verdict.

Every rejection is a loud, non-fatal diagnostic: `log::error!` (or `warn!` where noted) naming the reaction
and step index, the offending reaction dropped, the level and all other reactions unaffected — matching
`sequence_primitives_are_valid`, which already drops a whole reaction for one bad step. A dropped reaction
is left in place as an inert `Sequence(vec![])` rather than removed, so no later pass observes a shifted
vector. Hot reload inherits it: a bad edit costs one reaction and a console line, never the session.

| # | Pass | Authored | Response |
|---|---|---|---|
| V1 | A | `durationMs` zero, negative, NaN, non-finite, or overflowing `u32` ticks | `error!`, drop the reaction. Never a silent 1-tick wait (`NaN as u32` → 0 → `.max(1)`) or a `u32::MAX` countdown. The parse sites accept any JSON number; V1 is the only rejection point, because `manifest_from_js` degrades a `named_reaction_from_js` error to a `warn!` naming an array index, and `drain_global_reactions_js` propagates it with `?`, losing every mod-global reaction |
| V2 | B | `interruptible` wait in a reaction Enter-bound to a trigger whose `TriggerVolumeComponent.fire_mode` is `TriggerFireMode::Once` | `error!`, drop the reaction. Cancelling a `once` fire destroys the set-piece permanently — `update_after_fire` sets `latched`, and `evaluate_trigger_activation` rejects `Once if latched` |
| V3 | B | `interruptible` wait in a reaction with no trigger-Enter binding, counting **both** brush-KVP bindings and manifest `onTriggerEvent` bindings | `error!`, drop the reaction. The flag has no cancel source |
| V4a | A | Any step after a `wait` carrying a `SequenceTarget::Activators`/`FiredTrigger` sentinel | `error!`, drop the reaction. No fire context survives a wait. This clause needs only the `DataRegistry`, so it belongs in Pass A |
| V4b | B | Any `fire` step, at any position, whose target reaction's bound program reads a seeded dispatch input | `error!`, drop the reaction. A `fire` step dispatches on the app drain with no fire context regardless of position. **Pass B, not Pass A**: the predicate is over a `BoundProgram`, and none exists until `build_trigger_bindings` runs, after Pass A's slot. A post-wait *step* needs no equivalent check — the trigger binder's only `bind(...)` produces a `BoundStoreValue::Ir` for `setState`, and `bind_sequence_step` refuses `setState` outright, so a sequence step never yields a `BoundProgram` in any binder. The target of a `fire`, by contrast, may be exactly such a system-targeted `setState` `Primitive`, which is where `@rising` lives. Read it from `SystemReactionIrBindings`, whose `SystemSetStateBinding` carries `slot`, `value`, `program`, and `required_dispatch_inputs` but **no reaction identity**, with a private `bindings` field and no accessor: add the reaction name to that struct plus a `pub(crate)` accessor returning `(name, required_dispatch_inputs)`, and read that precomputed field rather than re-walking the tree — it already holds the answer, so the rule cannot go stale against a name list |
| V5 | B | `interruptible` wait on an Enter-bound reaction whose trigger emits no Exit edge | **derive it** — insert `(trigger, TriggerEventEdge::Exit)` into `bound_edges` via a new `bind_edge_only` method on `TriggerBindingTable`. `bind_event` cannot be reused: it returns early when `commands.is_empty() && steps.is_empty()`, before the `append_binding` call that is the only existing `bound_edges` inserter |
| V6 | A | `fire` step naming a reaction absent from the registry | `warn!`, drop the step, keep the reaction (mirrors the unknown-event `warn!` in `dispatch_deferred_named_events_with_sequences`) |

V5 is a derivation, not a rejection: without it the Exit arm of `run_authoritative_tick_with_dispatch`
`continue`s when `on_exit.is_empty()` and `bound_edges` lacks the pair, silently consuming the edge.

There is no duplicate-address rejection. Addressing is many-to-one (§12) and `bind_event` collects a
`matched` Vec, but each matched body dispatches separately and each `wait` enrolls its own instance, so two
same-named sequences park independently with no shared key to conflate.

## Ordering scenarios

Pin the orderings a task agent must handle. The test tasks cite these rows rather than restating them.

| # | Scenario | Ordering / input | Expected outcome |
|---|---|---|---|
| O1 | Basic delay | `levelLoad` sequence `[presA, wait(800), presB]` | presA runs at install inside `install_world_cpu`; the tail enrolls there. presB runs at the frame-end drain of the frame containing the 48th host tick, counting the first tick of the first frame after install as tick 1, where 48 = `max(1, ceil(800 * 1000 / 16_667))` |
| O2 | Sub-tick wait | `durationMs` = 5 | 1 tick; the tail resumes at the drain of the next frame that delivers a tick |
| O3 | Interrupt before elapse | interruptible instance; paired Exit at tick k < landing | remaining steps cancelled; consequence never applies |
| O4 | Interrupt on the landing tick | Exit on the tick the countdown would reach zero | cancel wins — cancels apply before countdown advance within `evaluate` |
| O5 | Non-interruptible during countdown | Exit while parked, `interruptible:false` | not cancelled; lands on schedule |
| O6 | Re-fire while parked (interruptible) | same `InstanceKey` re-fires before landing | prior instance cancelled, fresh instance from the top of the body |
| O7 | Re-fire while parked (non-interruptible) | same key re-fires before landing | re-fire ignored; running instance completes |
| O8 | Two instances land same tick | two parked instances resume on one tick | both land, ordered by ascending `InstanceId` |
| O9 | Wait crosses teardown | level unload while parked | instance dropped; no landing |
| O10 | Over the cap | concurrent instances exceed the per-level cap | excess enrollments `warn!` + drop; already-parked unaffected; the pre-wait steps have already run and the tail is abandoned |
| O11 | Multi-player plate | two players enter a per-player plate | one instance per key; each player's Exit cancels only their own |
| O12 | Enrollment never advances in its own frame | enrollment during frame F — at the pre-loop UI phase, the frame-end drain, or the crossing stage — then frame F+1 delivering 1, 3, or 14 ticks | the instance first advances on frame F+1's first tick, identically for all three phases and all three tick counts. The frame-counter stamp is what makes the pre-loop phase behave like the others |
| O13 | Multi-tick frame, short wait | 50ms frame → 3 ticks; `[playSound, wait(17), screenFlash]` fired on tick 0 | the pre-wait `playSound` runs in frame F's drain; the tail lands no earlier than frame F+1's drain, so authored order across the wait always holds |
| O14 | Zero-tick frame | frame with `ticks == 0` | no `evaluate`; countdowns unchanged; the landing queue still drains (it is outside the tick loop) but is empty |
| O15 | Max-ticks frame after a stall | 2s stall → accumulator clamps to `MAX_ACCUMULATOR` (250ms), ≤14 ticks (`floor(250ms / 16_667us)`) | `evaluate` runs once per tick; a countdown may reach zero mid-frame and lands at that frame's drain; wall-clock delay is the authored ms **plus** stalled time — lossy under stalls, stated |
| O16 | Two triggers, one reaction, one player | T1 and T2 both bind reaction R; P enters T1 at tick 10, T2 at tick 12 | two instances (key includes trigger); P's Exit from T1 cancels only T1's |
| O17 | Park at a later wait, Exit arrives | `[a, wait(200,i:true), b, wait(200,i:false), c]`; parked at wait1 when Exit lands | the wait currently parked at governs **both** Exit-cancel and re-fire — wait1 is non-interruptible, so no cancel; `c` lands |
| O18 | Re-fire while parked | same body; re-fire while parked at wait0 (interruptible) vs at wait1 (non-interruptible) | parked at wait0: accepted, restart from the top. Parked at wait1: ignored, exactly as O7. One rule — the parked-at wait governs — so O7 and O17 never disagree |
| O18b | Restart leaves later effects applied | accepted re-fire while parked at wait0, after an earlier run's `b` already wrote state | restart re-runs the body forward; effects already applied are **not** reverted. Authors write steps idempotent across restart; revert is a separate spec |
| O19 | Re-fire at the cap | cap full; same key re-fires | the same-key cancel is applied at enrollment before the cap test, so re-enrollment never counts against the cap and the pre-wait work is never stranded |
| O19b | Re-park at a later wait, cap full | `[a, wait(200), b, wait(200), c]` reaches wait1 while the cap is full | a nested wait inside a resumed tail re-enrolls under the same key; it holds the instance's existing slot and is never cap-tested, so `c` lands |
| O20 | Lifecycle step in a resumed tail | `[wait(1000), loadLevel("next")]`; two other instances parked | the tail runs at the drain, exactly where a trigger residual's `loadLevel` runs today; teardown at the next frame's `drive_boot_state_for_redraw` → `unload_level` drops the other instances |
| O21 | Landing consequence vs mover tick | `[wait(500), moverStart]` whose countdown reaches zero on tick k | the mover command runs at that frame's drain; first observable motion is the next frame's first tick — the same offset a trigger residual's `moverStart` produces today |
| O22 | Target despawned during the wait | `[wait(500), setAnimationState]` on E; E despawned before landing | `dispatch_sequence`'s existing `script_ctx.registry.borrow().exists(id)` guard skips the step with a warn; remaining steps in the tail still run |
| O23 | Activator dies or disconnects mid-wait | interruptible instance keyed to player P; P's pawn leaves the capsule set | the paired Exit fires and cancels — death or disconnect is a cancel source, identical to walking off |
| O24 | Connected client | client installs a level whose reactions contain waits | the client's own `install_world_cpu` runs the `levelLoad` fire and its pre-wait steps locally; enrollment is refused at the scheduler entry point with a host-only guard, so no tail parks and no tail runs client-side. Host consequences arrive by replication |
| O25 | Landing order determinism | N instances land on one tick, enrolled across different ticks, players, triggers | evaluation order is ascending `InstanceId`, never `HashMap` iteration order (matching `SlotAccumulatorBindings`'s explicit determinism comment); identical across runs for all N including 0 |
| O26 | Scheduler vs slot accumulators | a resumed `setState`-bearing reaction lands on tick k | the landing executes at the frame-end drain, *after* every tick's `evaluate_slot_accumulators` in that frame. An accumulator observes it on the next frame's first tick — the same offset a trigger residual's state write produces today |
| O26b | Resumed read of an accumulated slot | a resumed step reads a slot an accumulator writes | it reads the value settled at the end of the frame's last tick. A landed write to an accumulated slot bumps `write_generation` and rebases the accumulator's `precise_value`, discarding sub-f32 accumulation |
| O27 | `fire` step follow-ups | a resumed tail's `fire` step plus the same frame's residual follow-ups | one `dispatch_deferred_named_events_with_sequences` call for the trigger follow-ups plus one **per landing instance**, each with its own 256-hop budget — `MAX_BATCH_DISPATCH_HOPS` is a function-local const, not shared state. The per-frame ceiling is `(K + 1) * 256` for K landings, stated rather than discovered. Per-instance calls are what make depth attributable (O65) |
| O28 | Self-retriggering wait | R = `[x, wait(17), fire(R)]` bound to plate T; player P enters twice | the self-`fire` is sourceless, key `(R, ordinal, None)`; a trigger entry is key `(R, ordinal, Some((T,P)))`, so concurrency is one per key, not one total. The loop advances once per **frame-end drain**, not once per tick, and terminates at `MAX_REACTION_CHAIN_DEPTH` with a single warning naming the reaction — the concurrency cap alone never catches it |
| O28b | Depth resets on a fresh origin | a `fire` chain reaches depth 200; a player then enters a plate Enter-bound to the same reaction | the trigger-originated instance enrolls at depth 0 and is not dropped; depth is carried only along the enrolled-by relation |
| O29 | Malformed duration | `durationMs` = 0, -5, NaN, or 1e12 | V1: `error!`, reaction dropped, level continues; never a silent 1-tick wait or a `u32::MAX` countdown |
| O30 | `fire` a scoped reaction | `fire(r)` where `r` reads `on.rising` | TypeScript: compile error (`Reaction<{}>` parameter). Luau: V4b install rejection, at any step position |
| O31 | Enrollment from install | `levelLoad` `[presA, wait(17), presB]` enrolled inside `install_world_cpu` | the tail lands at the drain of the first frame that delivers a tick, identically for first frames of 1, 3, and 14 ticks |
| O32 | Enrollment from a UI or crossing fire | a segmented reaction fired by a focused button's `onPress`, `commit_text_entry`, or the host crossing stage | the tail enrolls through the same control arm and lands at a later frame's drain; no site-specific handling exists because none of these sites is enumerated |
| O33 | Landing residual identity | a landing's steps plus two trigger residuals in one frame | the scheduler owns its tails as `Vec<SequenceStep>` and never mints a `TriggerResidualHandle`; the drain never resolves scheduler tails through `trigger_bindings.residual()` and never executes another binding's steps |
| O34 | Name-fired reaction containing a wait | `S = [x, fire(R)]` where `R = [alarm, wait(800), moverStart]`; S fires | `fire_named_event_with_sequences` on `R` runs `alarm`, hits the `@wait` arm, enrolls the tail, and breaks — the door does not open in the same frame. This holds at every hop depth, because the arm lives below `dispatch_deferred_named_events_with_sequences`, not beside it |
| O35 | `setupLevel` validation admits control steps | a reaction containing `@wait`/`@fire` reaches `validate_sequence_primitives` | the reaction survives; no `names unknown primitive "wait"` error. `wait`/`fire` are registered names |
| O36 | Install-pass ordering | `onTriggerEvent({tag}, "enter", [reveal])` — a manifest binding installed by `install_manifest_events` | V2/V3/V5 run in Pass B, after `install_manifest_events`; the consumer's reveal is not dropped by V3, and V5's `bound_edges` insert survives |
| O37 | `fire` before a wait | `R = [fire(sting), wait(800), moverStart]` on a plate Enter | the `fire` arm pushes `sting` onto the dispatcher's returned chained list, so it reaches `dispatch_deferred_named_events_with_sequences` in the same frame's drain; it is never dropped by the `SequenceTarget::Entity` guard |
| O37b | `fire` order within one body | `R = [fire(sting), playSound("hum"), wait(800), moverStart]` | `hum` is heard before `sting`: a `fire` name is collected and dispatched after the whole body's presentation, matching how `chained` already behaves for `on_complete`. Authored order between a `fire` and a presentation step is **not** preserved — stated, not corrected |
| O38 | Teardown clears the scheduler | level unload with two instances parked | `clear_surface_lifetime_level_state` clears the scheduler's instance map and landing queue; no instance survives into level B holding level-A `EntityId`s |
| O39 | Suspend/resume mid-wait | window suspend while parked; `clear_surface_lifetime_level_state` runs via the suspend path with no level change | instances are dropped with a `warn!` naming the count, so a half-completed beat is distinguishable from a bug |
| O40 | Hot reload mid-wait | staged manifest commits while an instance is parked: `recompose_active_sets` → `rebuild_active_reaction_subscribers` → `rebuild_active_trigger_bindings` | parked instances are dropped with a `warn!` naming the count, and Pass A and Pass B both re-run in that commit block. The stored tail is a `Vec<SequenceStep>` owned by the scheduler, so a surviving instance would replay a body the author has edited away |
| O41 | Duplicate reaction address | two `defineReaction("levelLoad", { sequence: [...] })` bodies, one containing a wait | both dispatch; the one containing a wait parks its own instance keyed by name plus origin. Neither body is lost, and per-enrollment instances mean re-fire dedup does not conflate them |
| O42 | Co-op cue on the atmosphere channel | host fires the reveal; one connected client | the alarm reaction's `sharedGlobal` write replicates and the client's crossing fires a client-local presentation reaction; a client joining after the write observes the crossing once from its baseline |
| O43 | Client-local reaction containing a wait | a client-local crossing reaction whose body contains a wait | enrollment is refused at the host-only guard and the refusal is diagnosed once per reaction with a `warn!` naming it, so the dropped tail is visible rather than silent. See Owner decisions |
| O44 | Crossing stage is frame-sampled | a landing writes a watched slot, and another landing in the same frame writes it back | `run_crossing_stage` runs once per frame and compares settled values, so an intra-frame round trip fires no crossing. The consumer is authored so no opposing landing shares a frame |
| O45 | Landed spawn and the enrollment sweeps | `[wait(500), spawnFromSpawner]` landing at a frame's drain | the drain runs after that frame's mesh-clip resolve and `absorb_dynamic_lights` sweeps, so a spawned entity's clip binding and dynamic light are picked up on the following frame — the same offset a trigger residual's spawn produces today |
| O46 | Body whose first step is the wait | `R = [wait(800), moverStart]` bound solely by `onTriggerEvent` | the amended binder puts `[wait, moverStart]` into the residual, so `steps` is non-empty and `bind_event` does not early-return; the Enter edge binds normally and the residual enrolls at the drain. No `bind_edge_only` is needed for the Enter edge |
| O47 | `fire` step from a discarding call site | `defineReaction("levelLoad", { sequence: [fire(x)] })`, and the same body reached from a UI `on_press`, a text-entry `on_commit`, a death event, a mover sound event, and the crossing stage | `x` dispatches from every one. Each site captures the returned names and feeds `dispatch_deferred_named_events_with_sequences`; today all six discard the vec, so an uncaptured site is a silent no-op with no diagnostic |
| O48 | `onComplete` reactivation at those sites | a `Primitive` reaction with `on_complete` fired by `levelLoad`, a UI press, or a crossing | the chain now runs, where today it is dropped. Asserted so the behavior change is covered by a test rather than discovered as a regression |
| O49 | Trigger-bound wait survives the binder | `[fire(alarm), wait(800), moverStart, fire(release)]` Enter-bound to the plate | the pre-wait `fire` lowers to `DeferredEvent` and dispatches in the firing frame's drain; the wait and both following steps land in the residual in authored order; `moverStart` is **not** hoisted in-tick. Unamended, `classify` sends `moverStart` in-tick and the presentation branch drops the wait and both `fire` steps as sentinel targets |
| O50 | Pre-wait consequential stays in-tick | `[moverStart, wait(N), presB]` Enter-bound | `moverStart` binds as an in-tick `BoundTriggerCommand` and executes inside `simulate_tick`; only the wait and `presB` reach the residual. This is the E18-A guarantee, and the binder amendment is what delivers it |
| O51 | UI press enrolls before the tick loop | focused button `on_press` naming `[presA, wait(5), presB]`, in a frame delivering 3 ticks | `presA` runs pre-loop; the instance is stamped with the current frame counter, so none of that frame's three `evaluate` passes advances it. `presB` lands at the next frame's drain, identically to a crossing-fired enrollment |
| O52 | Enter and Exit in one frame, before enrollment | P enters the plate on tick j and exits on tick k >= j of the same frame; the residual enrolls at that frame's drain | the interruptible instance does **not** park: enrollment checks that its origin's paired enter is still standing, and P has already left. Without the check the cancel arrives before anything is parked, is lost, and the door opens with nobody on the plate |
| O53 | Exit between expiry and the drain | countdown reaches zero on tick j; paired Exit on tick k > j of the same frame; the tail is already in the landing queue | the cancel still applies — a queued-but-unrun landing is cancellable, and the queue is checked alongside parked instances. Sharpens O4, which pins only the exact landing tick |
| O54 | Two origins seed one deferred batch | P1 enters T1 and P2 enters T2 on the same tick; both residuals contribute `fire` names to one batch | both resulting instances are sourceless: the origin guard is scoped to the residual loop and released before the batch, so neither inherits the other's `(trigger, player)`. Their interruptible waits are demoted and warned per V3 |
| O55 | Sourceless demotion warns | a reaction that is both Enter-bound and a `fire` target, containing `wait(N, { interruptible: true })`, reached by the `fire` path | the sourceless instance treats the wait as non-interruptible and warns once at enrollment naming the reaction; the Enter-bound instance keeps its cancel source |
| O56 | Two same-named bodies re-fire | two `defineReaction("levelLoad", { sequence: [...] })` bodies, both containing a wait, both parked, then re-fired | each body's instance is keyed by its own `BodyOrdinal`, so neither same-key cancel touches the other and both restart. Keyed on address and origin alone, the second dispatch's cancel would destroy the first body's fresh instance and only one body would survive |
| O57 | Nested wait keeps its provenance | `R = [a, wait(200,i:true), b, wait(200,i:true), c]` Enter-bound to T by P; the first instance lands and the tail's second wait re-enrolls at the landing drain, outside the origin guard | the re-enrollment carries the instance's stored address, ordinal, and origin `(T,P)` — same `InstanceKey`, still interruptible, no demotion warning, no cap test. P's later Exit cancels before `c`. Re-deriving instead would make it sourceless, so V3's demotion would silently strip `interruptible` one wait into the body |
| O58 | `BodyOrdinal` survives a landing | two same-named bodies, each `[wait(17), x, wait(17), y]`, both parked, both landing on one tick | each tail re-enrolls under its own original ordinal, so both `y` steps run. The resume path has no `matched` loop, so a re-derived ordinal would be 0 for both and the second enrollment's same-key cancel would kill the first |
| O59 | One plate, two players, one residual handle | P1 and P2 enter T on the same tick; the binding's single `(trigger, edge)` residual handle is pushed once per fire | the drain enrolls two instances with distinct origins `(T,P1)` and `(T,P2)`, because `TickEvents.trigger_residuals` carries the trigger and player beside each handle. Carrying the handle alone yields `[h, h]` with no player in scope and collapses both to one instance |
| O60 | Paired-enter check with a bystander | P1 enters on tick j and exits on tick k of frame F; P2 has stood on T since frame F−3 | P1's enrollment is refused and P2's is unaffected: the check is `paired_enters` membership for `(T, P1)` specifically, never "is anyone on T". Keyed per trigger it would pass on P2's presence and fire the beat for a player who left |
| O61 | Headless frame stamping | `SimHarness` installs `levelLoad = [presA, wait(5), presB]`, then calls `frame` twice, each supplying one command | `begin_frame()` advances the scheduler's counter once per `frame()`, so `presB` lands at the second frame's drain. A counter advanced only by `RedrawRequested` never advances here and the instance is skipped forever |
| O62 | Resumed tail vs the residual assertion | the scheduler lands `[moverStart, fire(release)]` — a tail containing no `Wait` — in a debug build | no `debug_assert!` fires, because the exemption keys on `ResidualOrigin::ResumedTail`. A content-based rule exempting residuals that contain a `Wait` would still panic here, since a post-wait tail never contains one |
| O63 | Trigger removed mid-wait | an interruptible instance is parked on `(T,P)`; `T` leaves `active_triggers` before the wait elapses | `paired_enters.retain` drops the pair and no Exit ever fires, so the instance would land uncancelled. The scheduler drops any instance whose keyed trigger is gone, with a `warn!` naming it — the install-time guarantee that an interruptible wait has a cancel source does not survive trigger removal on its own |
| O64 | Cap holds a landing instance's slot | cap full; K instances expire on one tick; the residual drain then enrolls new trigger fires before the landing drain runs | an instance occupies its slot from enrollment until its final segment completes, so the freed-at-expiry slots are not available to that frame's new fires and every nested re-enrollment still lands. Freeing at expiry strands K tails |
| O65 | Depth across a multi-landing batch | two instances at depths 3 and 7 land on one tick and each `fire`s a segmented reaction | each enrollment inherits its own parent's depth + 1 — 4 and 8 — because depth rides the instance and the step, not a batch-scoped cell. A per-batch value gives both the same wrong depth |

## Acceptance criteria

- [ ] A `levelLoad` sequence `[presA, wait(800), presB]` runs presA at install and presB at the drain of the
  frame containing the 48th host tick, in a headless N-tick harness (O1, O31).
- [ ] A 5ms wait resolves to 1 tick and never resumes in the frame that enrolled it (O2, O12).
- [ ] An interruptible instance whose paired Exit arrives before the countdown elapses cancels its remaining
  steps (O3); an Exit on the exact landing tick also cancels (O4).
- [ ] `interruptible:false` ignores the paired Exit during countdown; remaining steps land on schedule (O5).
- [ ] Re-firing an interruptible instance while parked cancels and restarts from the top; re-firing a
  non-interruptible one is ignored (O6, O7). A re-fire at a full cap still re-enrolls (O19), and a nested
  wait re-parks without a cap test (O19b).
- [ ] A delayed `fire` step dispatching a system-targeted `setState` reaction writes a `network: "shared"`
  slot that reaches a connected client through state replication, with no client-side scheduler evaluation
  (O24). It must be a `fire` step, not a `setState` step: `setState` is registered only on the
  `SystemReactionRegistry`, never by a `register_sequenced_*` call, so a `setState` sequence step is dropped
  whole at `setupLevel`. It must not be `moverStart` either: mover phase replicates as its own
  `kinematic_mover` component record on the entity snapshot rather than through `state_records`, and neither
  named harness emits one. The invariant under test — host-authoritative, replicated, never client-re-simmed
  — holds on the state-slot path.
- [ ] A pre-wait consequential step bound to a trigger still executes inside the firing tick: a trigger-fired
  `[moverStart, wait(N), presB]` starts the mover within `simulate_tick`, with no app drain (O50, E18-A
  invariant).
- [ ] A trigger-bound body containing a wait survives install with the wait and every following step intact
  in the residual, and no post-wait consequential step hoisted in-tick (O49).
- [ ] An instance enrolled before the tick loop by a UI `on_press` does not advance during that frame's
  ticks (O51), and an interruptible instance whose activator has already left by enrollment time does not
  park (O52). A cancel arriving after expiry but before the drain still cancels (O53).
- [ ] Two `fire` names seeded by different residuals into one deferred batch both enroll sourceless, and the
  demotion warns once (O54, O55).
- [ ] Two same-named bodies both containing a wait re-fire without cancelling each other (O56), and still
  do not conflate after each has landed once and re-parked at a nested wait (O58).
- [ ] A nested wait in a resumed tail keeps its instance's address, ordinal, and origin: it stays
  interruptible, warns nothing, and re-parks successfully with the concurrency cap already full (O57).
- [ ] Two players entering one plate on the same tick yield two instances with distinct origins, and each
  player's Exit cancels only their own (O59) — including when a third player is standing on the plate
  throughout (O60).
- [ ] Both headless drivers advance the scheduler's frame counter via `begin_frame()`, so an enrolled
  instance is not skipped forever; and in `SimHarness`, which has the frame-end drain, the wait actually
  lands (O61). `run_headless_inner` has no drain, so it proves only the counter half.
- [ ] A resumed tail containing `moverStart` runs in a debug build without tripping either residual
  assertion (O62).
- [ ] An interruptible instance whose keyed trigger is removed mid-wait is dropped with a `warn!` rather than
  landing uncancelled (O63).
- [ ] With the cap full, K instances expiring on one tick still complete their tails even though the residual
  drain enrolls new fires first (O64); and two instances landing at different depths each seed enrollments at
  their own depth + 1 (O65).
- [ ] No VM invocation occurs inside `simulate_tick` on any tick where a countdown reaches zero. Assert this
  structurally, not by log absence: `dispatch_sequence` emits no success-path marker today, and
  `assert_not_logged` proves absence across the whole test process rather than inside one phase. The
  structural proof is the `ResidualOrigin::TriggerBinding` assertion (Task 3) plus the pre-wait in-tick
  criterion above — together they show the only in-tick path is bound commands. Mark as a review gate, not a
  runnable test.
- [ ] Level unload/restart drops all pending instances; no step lands after unload (O9, O20, O38).
- [ ] A suspend-path clear drops parked instances with a `warn!` naming the count (O39), and a staged hot
  reload does the same and re-runs both validation passes (O40).
- [ ] Concurrent instances past the per-level cap warn and drop; already-parked instances are unaffected (O10).
- [ ] A self-`fire` loop terminates at `MAX_REACTION_CHAIN_DEPTH` with a single warning naming the reaction; the
  level keeps running and unrelated instances are unaffected (O28), and a fresh trigger fire enrolls at depth
  zero (O28b).
- [ ] A per-player plate entered by two players yields two instances; each Exit cancels only its own (O11).
- [ ] Two instances landing on one tick both land in ascending `InstanceId` order (O8), and two headless runs
  with identical inputs produce identical landing frames and identical final state (O25).
- [ ] Each Install validation row V1, V2, V3, V4a, V4b drops exactly the offending reaction with a diagnostic naming the
  reaction and step index; the level installs and every other reaction is unaffected.
- [ ] V5: an interruptible wait on an Enter-bound reaction causes the install binder to register the trigger's
  Exit edge, and cancellation works with no authored `on_exit` KVP.
- [ ] A `fire` step dispatches its named reaction in the drain its body ran in; `fire()` accepts a reaction
  handle and a string, and a missing target warns without dropping the reaction (V6, O37).
- [ ] A `fire` step dispatches from the two named-fire sites a harness can reach — `levelLoad` at install
  (`run_headless_inner`) and `dispatch_state_crossings_with_sequences` (the trigger state-channel harness) —
  and an `on_complete` chain fired from them now runs (O47, O48). The remaining four sites live in `App` with
  no harness; cover them with a grep gate instead: no **non-test** caller of `fire_named_event_with_sequences` discards
  its return value. Scope the gate by path rather than keeping an allowlist — two `#[cfg(test)]` callers
  legitimately discard, and an allowlist goes stale the moment a third appears.
- [ ] `wait` and `fire` do not trip `sequence_primitives_are_valid`; a sequence naming an unknown *action*
  primitive is still rejected as before (O35).
- [ ] The same timed sequence authored in TypeScript and in Luau produces the same landing frame index and the same final slot state — a loose "same behavior" check would pass a Luau builder that defaulted `interruptible` to `true`.
- [ ] The typedef drift check passes with regenerated `sdk/types/postretro.d.ts` / `.d.luau`, and snapshot
  typedef tests in `crates/postretro/src/scripting/typedef/tests/` cover the `wait` and `fire` step shapes.
- [ ] `sdk/lib/index.ts` re-exports both builders so `import { wait, fire } from "postretro"` resolves in the
  consumer fixture — a separate surface from the typedefs, which `sdk/type-tests/tsconfig.json` resolves
  through `../types/postretro.d.ts`.
- [ ] Firing a reaction whose body contains a wait by name runs only up to that wait, at every hop depth
  at hop depth >= 1 inside `dispatch_deferred_named_events_with_sequences` — reached from a deferred batch
  rather than a direct fire (O34) — and the same holds from a crossing fire (O32). The UI half of O32 has no
  harness; cover it as a review gate.
- [ ] The scheduler never mints a `TriggerResidualHandle` (O33) — a grep gate over the scheduler module, not a runnable test.
- [ ] Two sequences sharing one address, either containing a wait, park independent instances — neither body
  is lost and re-fire does not conflate them (O41).
- [ ] V2/V3/V5 run after `install_manifest_events`: the consumer's manifest-bound reveal is not dropped by
  V3, and its derived Exit edge survives (O36). A non-interruptible wait in a body with no pre-wait work
  still dispatches its Enter edge (O46).
- [ ] Parked-at-wait governs both cancel and re-fire: a re-fire while parked at a non-interruptible wait is
  ignored, and one while parked at an interruptible wait restarts from the top without reverting effects
  already applied (O17, O18, O18b).
- [ ] A client-local reaction containing a wait warns once and drops its tail rather than failing silently
  (O43).
- [ ] A `sdk/type-tests/` `@ts-expect-error` case proves `fire()` rejects a scoped reaction, alongside the
  existing `invalidScopeErasure` case in `sdk/type-tests/e18-dispatch-params.ts` (O30).
- [ ] Consumer (engine-run fixture check on a compiled map, not `cargo test`): on the closet-reveal fixture the plate's `enter` raises the alarm immediately; the door does not
  start and the enemies are not released until after the wait; leaving the plate during the wait leaves the door
  closed and the enemies contained; and the plate can be re-entered to run the beat again.
- [ ] The consumer's alarm reaches a connected client in a two-endpoint harness: the host's `sharedGlobal`
  write replicates and the client's crossing fires its local light reaction, so both players see the cue (O42).

## Tasks

### Task 1: Control steps, the dispatch arm, and the scheduler — thin slice

Add the control steps and the timing seam end to end for a `levelLoad`, presentation-only sequence,
falsifying descriptor → deserialize → dispatch arm → scheduler → landing frame before any fan-out. Wire
shapes are `{ id: "@wait", primitive: "wait", args: { durationMs, interruptible } }` and `{ id: "@fire",
primitive: "fire", args: { event: string } }` (Boundary inventory). Extend `sequence_steps_from_js`
(`crates/scripting-core/src/data_descriptors/js/reactions.rs`) and its twin `sequence_steps_from_lua`
(`.../lua/reactions.rs`) to accept the `"@wait"` and `"@fire"` sentinels — today every spelling other than
`"@activators"`/`"@trigger"` returns `DescriptorError::InvalidSequenceShape`. Accept any JSON number for
`durationMs` here, and note both deserializers default a missing `args` to `serde_json::Value::Null` rather
than `{}`, so V1's reader must tolerate `Null` and not only a missing key. V1 in Pass A is the sole
rejection point, because these sites cannot drop one reaction
cleanly (see the V1 row). Add **two** `SequenceTarget` variants, `Wait` and `Fire`, both payload-free so
the enum keeps its `Copy` derive — all payload stays in `args`. Enumerate and amend the match sites, and
note which the compiler catches: `bind_sequence_step` (`crates/postretro/src/trigger_bindings.rs`) is an
exhaustive `match` over `step.id` with no wildcard and **will** fail the build; give it an early
`return None` for `Wait | Fire` ahead of the `target` construction, since `BoundTarget` has no analogue —
the amended `partition_direct_reaction` routes `Fire` to `DeferredEvent` before reaching it; `dispatch_sequence` (`let …else`),
`partition_direct_reaction` (`!matches!`), `reaction_uses_trigger_sentinel`
(`crates/postretro/src/startup/lifecycle.rs`, `!matches!`), and `runtime_sequence_step_shape_is_valid`
(`crates/script-compiler/src/light_membership.rs`) are all guards the compiler will **not** flag, and each
fails open if unamended. The last accepts only `"@activators" | "@trigger"`, and its caller skips the whole
sequence when any step fails it — so a body containing a `wait` silently reserves no light-bake slot for its
`setLightAnimation` steps, with no diagnostic. Add the new sentinels to its accepted set. `reaction_uses_trigger_sentinel` is load-bearing: it feeds
`rebuild_reaction_subscribers` to strip crossing subscriptions and to warn that "levelLoad references
trigger-sentinel work," so a control step must not read as a trigger sentinel there.
Add the control arm to `dispatch_sequence`, **ahead of** its `SequenceTarget::Entity(id)` guard: on `Wait`,
call the registered wait handler with `&steps[i + 1..]` and `&step.args`, then `break`; on `Fire`, push the
`event` name onto a collected list. `dispatch_sequence` today is
`fn dispatch_sequence(steps: &[SequenceStep], sequence_registry: &SequencedPrimitiveRegistry, script_ctx: &ScriptCtx)`.
Change it to return `Vec<String>` of collected `fire` names, and have `fire_named_event_with_sequences` and
`fire_prepartitioned_reactions_with_sequences` extend their existing `chained` vec with them.
**Also add `address: &str` and `body_ordinal: usize` parameters**, and pass both to the control handler —
they are the instance key's first two components and nothing downstream can reconstruct them.
`fire_named_event_with_sequences` supplies the address from `named.name` and the ordinal from a running
count of same-named matches in its `for named in &data_registry.reactions` loop.
`fire_prepartitioned_reactions_with_sequences` takes both from the step: Task 3 widens
`PrepartitionedReactionStep::Descriptor` to carry them, because that function has no name in scope. The
control-handler signature is therefore `(&str /* address */, usize /* body ordinal */, &[SequenceStep],
&Value)`.
It is **not** sufficient on its own: only the residual path currently consumes a returned `chained` vec.
Every production caller of `fire_named_event_with_sequences` discards it — one as a bare statement (the
`levelLoad` fire) and five as `let _ =` — so a `fire` step in a body reached from any of them would
dispatch nothing. Capture
and feed the returned names into `dispatch_deferred_named_events_with_sequences` at all six: the
`levelLoad` fire in `install_world_cpu` (`startup/lifecycle_world_cpu.rs`),
`dispatch_state_crossings_with_sequences` (`crates/postretro/src/scripting/reactions/mod.rs`),
`drain_mover_sound_events_with_sequences`, the `pending_death_events` loop, the
`UiButtonAction::NamedReaction` `on_press` path, and the `commit_text_entry` `on_commit` path (the last
four in `main.rs`). Two further call sites are test-only setup (`scripting/reactions/mod.rs` and
`fx/fog_reactions/mod.rs`) and need no change. This activates `PrimitiveDescriptor.on_complete` chaining at
those six sites, where it is silently dropped today — a deliberate behavior change, not a side effect to
discover during implementation (Owner decisions).
Register `"wait"` and `"fire"` in `SequencedPrimitiveRegistry` so `sequence_primitives_are_valid` /
`validate_sequence_primitives` / `validate_scoped_sequence_primitives` admit them — those functions iterate
every step and reject any `primitive` absent from the registry, dropping the whole reaction at `setupLevel`.
Those validators consult `contains`, which reads only the `handlers` map, so registration must land there:
give each an **inert** `handlers` entry purely to satisfy admission, and put the real behavior in a parallel
control-handler table on the same registry, whose signature is the one pinned below —
`(&str /* address */, usize /* body ordinal */, &[SequenceStep], &Value)` — because `SequencedPrimitiveFn` is `Box<dyn Fn(EntityId, &serde_json::Value) -> Result<(),
SequenceError>>` and can receive neither the address nor the tail. Register the control table from
`session/mod.rs` capturing a `ReactionScheduler` clone. The control arm consults the control table and never
reaches the inert `handlers` entry; state that, so the two registrations do not read as contradictory. Add `ReactionScheduler` in a new
file under `crates/postretro/src/scripting/systems/`, shaped exactly like `MoverAutoCloseTimers`
(`crates/postretro/src/kinematic_mover/auto_close.rs`): `#[derive(Debug, Clone, Default)]` over an
`Rc<RefCell<_>>`, main-thread only, owned on the session beside `slot_accumulator_bindings` and cloned into
the handler at registration. Store instances in a `BTreeMap` or insertion-ordered container keyed by
`InstanceKey` and sorted for evaluation by the monotonic `InstanceId` (O25 and the `SlotAccumulatorBindings`
determinism precedent). Stamp each instance with a **monotonic frame counter** and have `evaluate` skip any instance whose stamp
equals the current frame — a UI `on_press` dispatches ahead of the tick loop in the same redraw, so without
the stamp a `wait(5)` enrolled there would resume in its own frame (Enrollment, origin, and identity). The
counter advances through a scheduler-owned `begin_frame()` with **three** call sites, and naming all three
is load-bearing: the `RedrawRequested` arm in `main.rs`, `SimHarness`'s new `frame` method, and
`run_headless_inner`'s loop. **Task 1 adds `frame` itself** — it runs n `tick()`s and then the
frame-end drain (residual loop, then the scheduler landing queue, then the deferred hops) — because Tasks 1,
3, 5 and 7 all need a frame and Task 1's own acceptance criteria are written against it. `SimHarness`
(`crates/postretro/src/sim/determinism_tests.rs`) resolves no residuals today, and its whole module is
private — the declaration is `#[cfg(test)] mod determinism_tests;`, so widening only the struct leaves it
unreachable. Make the **module** `pub(crate)` along with `new`, `tick`, `record`, and `frame`, and give
`new` a fixture parameter: Task 5's rows need two players across one or two plates, which the current fixed
fixture cannot express. It has exactly one `new` call site, so the blast radius is small. `frame` cannot
simply loop `tick`: `tick(&mut self, command: RecordedCommand) -> RecordedTick` requires a per-tick command,
which in a determinism harness is the load-bearing input — give `frame` a signature that supplies them
(`frame(&mut self, commands: &[RecordedCommand])`). Disambiguate by path everywhere: a second, unrelated
`SimHarness` lives in `crates/postretro/src/sim/divergence_spike_tests.rs`. A counter advanced only by window events never advances in either headless
driver, where every instance would be skipped on every pass and nothing would ever land — and those drivers
are exactly where the O1/O2/O31 acceptance criteria are written. `run_headless_inner` has no frame concept,
so it calls `begin_frame()` once per tick and lands a 1-tick wait on the following tick; state that its
offset is defined rather than incidental. Do **not** use `tick_index`: it resets every frame
(`for tick_index in 0..ticks`). Do not use `NetEndpoint::Host { tick }` either — it advances only when a net
endpoint exists, so it is absent in single-player. Guard enrollment host-only at the scheduler entry point,
not at a call site, since named fires run on clients too (O24, O43). Gate the scheduler with a `set_enabled(bool)` latch, matching `MoverAutoCloseTimers` and set at the same
place: `session/mod.rs` resolves `net_endpoint` and then writes three parallel latches
(`spawn_context.set_runtime_spawn_authority`, `auto_close_timers.set_enabled`,
`script_ctx.owner_slot_writes_enabled.set`), each `!matches!(&net_endpoint, Some(NetEndpoint::Client { .. }))`.
Add a fourth beside them. `is_connected_client` is an `App` method, not a session one, so the scheduler
cannot read it per call regardless — something must write a cell, and this is where the role is known.
Follow the precedent's clear-on-disable too. That latch is also the **only** seam a test can use to drive
the client-refusal rows: no harness constructs a `NetEndpoint`, so O24 and O43 call `set_enabled(false)`
directly, exactly as `auto_close.rs`'s own tests call `set_enabled`. Countdowns are whole-tick integers, so `evaluate` takes no `dt`. Convert ms to ticks
with integer micros against `TICK_DURATION`, which is `pub(crate)` in the `postretro` binary and invisible
to `scripting-core`: `durationMs` stays in the descriptor and the conversion happens **at enrollment**,
inside the control handler, which already lives in `postretro`. Pass A only validates the value; it does not
convert. Wire `evaluate` into the host tick loop in `main.rs` and into
`run_headless_inner` (`observability/driver.rs`) and `SimHarness` (`sim/determinism_tests.rs`). Its position
relative to `evaluate_slot_accumulators` is not behaviourally load-bearing — landings execute at the
frame-end drain, after every tick's accumulator pass (O26) — so place it immediately after
`simulate_tick_with_presentation_aim` and state that. Unit tests, each row glossed because the ordering table is not in the executor's payload: O1 (48-tick
landing), O2 (5ms resolves to 1 tick), O12 (an enrollment never advances in its own frame), O14 (a
`ticks == 0` frame runs no `evaluate`; countdowns unchanged; the landing queue drains but is empty), O31
(install-time enrollment lands on the first frame that delivers a tick), O34 (a name-fired body runs only up
to its wait, including at hop depth >= 1 inside a deferred batch), O35 (`wait`/`fire` survive `setupLevel` validation), O41 (two same-named bodies park
independently), O51 (a UI-press enrollment does not advance during its own frame), O61 (both headless
drivers advance the frame counter).

### Task 2: SDK `wait()` and `fire()` builders and typedefs

Add both builders to the SDK, following the full `armTrigger` chain in `sdk/lib/data_script.ts` (each
returns a one-element `SequenceStep[]` with a sentinel id).
`wait(durationMs: number, opts?: { interruptible?: boolean }): SequenceStep[]` emits `{ id: "@wait", primitive:
"wait", args: { durationMs, interruptible: opts?.interruptible ?? false } }`. `fire(reaction: Reaction<{}> |
string): SequenceStep[]` emits `{ id: "@fire", primitive: "fire", args: { event } }`, resolving the name
exactly as `onTriggerEvent` already does — `typeof reaction === "string" ? reaction : reaction.name` — so a
handle works whether its id was authored or derived by `autoReactionId`. Typing the parameter `Reaction<{}>`
(not `Reaction<S>`) makes firing a scoped reaction a compile-time error via the `reactionScopeBrand`
phantom, which is §12's author-facing gate; Luau relies on the V4b engine gate. The chain has ten links and
all ten are required — the `@ts-expect-error` AC cannot compile without the template function declarations,
and the Script syntax example's import cannot resolve without the root re-export: (1) builders plus
`WaitStep`/`FireStep` union members in `sdk/lib/data_script.ts`; (2) Luau twins in
`sdk/lib/data_script.luau`; (3) re-exports in `sdk/lib/index.ts`, which carries the comment "When adding
public root exports here, also update TS_SDK_LIB_BLOCK and LUAU_SDK_LIB_BLOCK"; (4) `WaitArgs`/`FireArgs`
interfaces, the step types, the union entries, **and** `export function wait/fire` declarations in
`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts`; (5) the `.luau` template twins; (6) `wait`/`fire`
entries in the per-function export table in
`crates/scripting-core/src/typedef/templates/virtual_module.luau` — without them a new global is unreachable
from a Luau mod; and three runtime inventories that the typedef comment does not mention and that fail the
build if missed — (7) `DATA_SCRIPT_FIELDS` and (8) `POSTRETRO_ROOT_MODULE_EXPORTS` in
`crates/scripting-core/src/luau_prelude.rs`, each guarded by a test asserting exact-set equality against the
evaluated `.luau` surface, and (9) the hand-written root list inside `assert_exact_string_keys` in
`crates/scripting-core/src/luau_require.rs`. Missing (7) or (9) fails a test; missing (8) leaves
`require("postretro").wait` nil at runtime while the typedef still declares it. And (10) `const DATA_FIELDS`
in `crates/script-compiler/src/light_membership.rs`, a hand-written list feeding both the globals and the
`require("postretro")` root of the light-membership pre-pass that evaluates author Luau at level-compile
time — without it a Luau `wait(...)` is nil during `prl-build`, so the both-runtime parity criterion fails
in a crate no other link touches. Regenerate typedefs and update the committed/snapshot tests in
`crates/postretro/src/scripting/typedef/tests/`. Add a `@ts-expect-error` negative case to
`sdk/type-tests/`, beside the shipped `invalidScopeErasure` case in `sdk/type-tests/e18-dispatch-params.ts`,
proving `fire()` rejects a scoped reaction (O30); that file's `tsconfig.json` sets `strict: true`, which is
what makes the contravariant brand bite. Both-runtime parity fixture: the same timed sequence in TS and
Luau. Depends on Task 1 only for the settled wire shapes in the Boundary inventory.

### Task 3: Binder partition, resume execution, lifecycle, and cycle breakers

**Amend the binder so a trigger-bound wait survives install.** `partition_direct_reaction`'s
`ReactionDescriptor::Sequence` arm (`crates/postretro/src/trigger_bindings.rs`) today walks every step and
binds each one `classify` calls `Consequential` into an in-tick command **regardless of position**, while
its presentation branch drops any step whose `id` is not `SequenceTarget::Entity(_)` with a
`"sentinel target on presentation sequence step"` warn. `classify` returns `Presentation` for `wait` and
`fire`, and both carry non-`Entity` ids, so unamended it deletes the wait and both `fire` steps from the
consumer's body and binds `moverStart` in-tick — the door opens in the firing tick. Amend the arm to stop at
the first `Wait`: steps before it partition exactly as today (consequential to in-tick commands, a `Fire` to
`PrepartitionedReactionStep::DeferredEvent`, presentation to the residual), and the `Wait` plus every step
after it goes into the residual `Descriptor(Sequence(..))` **in authored order, unfiltered by class or
target**. This is what makes the pre-wait in-tick AC true and what delivers the wait to `dispatch_sequence`
at the frame-end drain. Widen `PrepartitionedReactionStep::Descriptor` to carry
`(address: String, body_ordinal: usize)` alongside the descriptor. `partition_direct_reaction` has no ordinal
in scope, so give it an explicit `body_ordinal: usize` parameter fed from `bind_event`'s
`matched.iter().enumerate()` — `matched` filters to a single name, so that index agrees with the count
`fire_named_event_with_sequences` derives from the same `data_registry.reactions` order. Widening the shared
variant also breaks two pattern-matches that deserve the same enumeration `WorldInstallHandles` gets: one in
`crates/postretro/src/trigger_bindings.rs`'s tests and one in
`crates/postretro/src/netcode/trigger_state_channel_harness_test.rs`, both matching
`Descriptor(ReactionDescriptor::Primitive(`. The `Primitive` arm carries the pair without reading it, which
is accepted rather than splitting the variant — that pair is what `fire_prepartitioned_reactions_with_sequences` hands to `dispatch_sequence`'s
address and ordinal parameters. Do **not** put an address on `TriggerResidual`: `bind_event` partitions all
of a `(trigger, edge)`'s matched reactions into one `commands`/`steps` pair before calling `append_binding`,
which further merges into any existing residual for that pair, so one residual is N reactions' product and a
single address field there is wrong.

Then execute a resumed tail and make the scheduler survive the level lifecycle. On expiry the scheduler
moves the instance's stored `Vec<SequenceStep>` into a scheduler-owned landing queue; `main.rs` drains that
queue after the trigger follow-up dispatch that already follows the `pending_trigger_residuals` loop — not
between the loop and that dispatch — by wrapping each tail in
`PrepartitionedReactionStep::Descriptor(ReactionDescriptor::Sequence(tail))` and calling
`fire_prepartitioned_reactions_with_sequences` — the same `pub` call the residual loop already makes.
**Relax the two `#[cfg(debug_assertions)] debug_assert!` guards** on that function's
`Descriptor(ReactionDescriptor::Sequence(..))` and `Descriptor(ReactionDescriptor::Primitive(..))` arms —
two guards with two distinct messages, both asserting `!is_trigger_consequential_primitive(..)`.
`moverStart` is in that 13-name list, so the consumer's own tail `[moverStart, fire(release)]` panics every
debug build without this. Key the exemption on **who is calling, not on what the steps contain**: add a
`ResidualOrigin { TriggerBinding, ResumedTail }` parameter and assert only for `TriggerBinding`. A
content-based rule — "exempt a residual containing a `Wait`" — exempts the trigger residual
`[wait, moverStart, fire]` and still panics on the resumed tail `[moverStart, fire]`, which by construction
is everything *after* a wait and therefore never contains one. That is precisely the case the feature
exists to produce. The guards exist to catch consequential work draining app-side when the binder should
have bound it in-tick; for a resumed tail that draining is legitimate. `is_trigger_consequential_primitive`
is itself `#[cfg(debug_assertions)]`, so any helper must be gated the same way. The parameter touches four
sites across two crates — the `main.rs` residual drain (`TriggerBinding`), the new `main.rs` landing drain
(`ResumedTail`), a `#[cfg(test)]` caller in `crates/postretro/src/sim/mod.rs`, and an in-crate test in
`crates/scripting-core/src/reaction_dispatch.rs` (both `TriggerBinding`). It is read only under
`#[cfg(debug_assertions)]`, so keep it live in release. State the rationale in the code comment. **Drain the landing queue one instance at a time, and give each instance its own
`dispatch_deferred_named_events_with_sequences` call**, after the trigger follow-up dispatch and outside the
origin guard — never merged into `pending_trigger_follow_ups`, and never one call for the whole queue. This
is the only structure that can attribute depth: the deferred queue is a `VecDeque<String>` inside
`scripting-core` with no per-name metadata slot, and the control handler receives no depth, so the only
place depth can live at enrollment is a scheduler cell — and a cell is correct only while exactly one
instance's follow-ups are in flight. Set `current_enrollment_depth` with an RAII guard around each
instance's `fire_prepartitioned_reactions_with_sequences` call and its own deferred dispatch, exactly as
Task 5 scopes `current_origin`. One call for the whole queue would give every enrollment a single depth,
wrong whenever K instances land at differing depths (O65). Each call carries its own 256-hop budget, so the
per-frame ceiling is `(K + 1) * MAX_BATCH_DISPATCH_HOPS` for K landings, not a fixed 512 (O27). No class partition and no
`BoundTriggerCommand::execute` plumbing: a resumed step runs where a trigger residual's step runs.
A nested `wait` inside a tail re-enters the Task 1 control arm and re-enrolls under the same key, so
multi-wait bodies need no separate path (O17, O19b). Do **not** append to `pending_trigger_residuals`: it is
a `Vec<TriggerResidualHandle>` resolved through `trigger_bindings.residual(handle)`, a bare index into
`TriggerBindingTable::residuals`, so a scheduler entry would either fail to resolve or silently execute
another binding's steps (O33). Mind the borrow: that drain block sits inside
`if let Some(session) = self.session.as_ref()`, a shared borrow, and resolves handles through the `App`
field `self.trigger_bindings` — take the landing queue with `std::mem::take` into a local before the block,
or place the queue on `App` beside `trigger_bindings`; pick one and state it. Register the scheduler's clear
in `clear_surface_lifetime_level_state` (`crates/postretro/src/startup/lifecycle_net.rs`) — that function
enumerates its clears field by field, so an unregistered structure survives a level change — and note it is
also the **suspend** path, so a suspend mid-wait drops parked instances with a `warn!` naming the count
(O38, O39). Register the same drop in the staged-manifest commit block
(`crates/postretro/src/startup/staged_manifest_lifecycle.rs`, `poll_staged_manifest_results`), because a
stored tail is a snapshot of a body the author may have just edited (O40). That block runs **four** calls in
order — `recompose_active_sets`, then `rebuild_active_reaction_subscribers`,
`rebuild_active_system_reaction_bindings`, `rebuild_active_trigger_bindings` — and the third rebuilds the
binder V4b's `fire`-target half reads, so it cannot drop out of the chain. Pin the insertion points: Pass A
runs after `recompose_active_sets` and **before** `rebuild_active_trigger_bindings`, or the binder binds a
body Pass A rejects; Pass B runs after both binder rebuilds. The `session` borrow there is scoped to the
`recompose_active_sets` call, so the scheduler drop and each pass need their own. Leave the validation-pass re-run at that same
point to Task 4, which owns both passes. `WorldInstallHandles` (`startup/lifecycle.rs`) gains a field if the
scheduler is threaded through install — amend its **four** construction sites: three in
`startup/lifecycle.rs`, of which only the first is production and the other **two** sit inside that file's
`#[cfg(test)] mod tests` (which runs to end of file), plus one in `run_headless_inner`
(`crates/postretro/src/observability/driver.rs`), plus the destructure in `install_world_cpu`. Enforce a
per-level cap on concurrent instances (`MAX_PENDING_REACTION_INSTANCES = 256`, a named const in the
scheduler module; warn + drop past it, noting the pre-wait steps have already run and the tail is
abandoned). Size it as a cycle breaker, not a content budget: four players across thirty timed plates is 120
legitimate concurrent instances, so a smaller cap would bite authored co-op content while a runaway grows
without bound either way. Carry a chain-depth counter alongside it, as a second named const in the same scheduler module. **Depth
travels on the instance and the step, exactly as address and ordinal do — never in a batch-scoped cell.**
One landing batch carries `fire` names seeded by several landings at different depths (O8 admits K
simultaneous landings), so a per-batch depth misattributes for the same reason a per-batch origin does. An
instance enrolled by a `fire` step in another instance's landing inherits depth + 1; past
`MAX_REACTION_CHAIN_DEPTH = 256` the enrollment warns once and drops. Depth is carried only along the
enrolled-by relation, so a fresh trigger fire starts at zero (O28b), and a **nested** re-enrollment keeps
its instance's depth unchanged — a body with 300 sequential waits is not a chain (O19b).
**Cap accounting:** an instance occupies its slot from enrollment until its final segment completes, not
until expiry — the residual drain runs before the landing drain in the same frame, so slots freed at expiry
would be refilled by that frame's new trigger fires and the nested re-enrollment would then fail the cap,
stranding every tail. Exempt a nested re-enrollment by rule rather than by "holds its slot" (O19b): the landing drain already
runs per instance, so set a scheduler `currently_resuming` cell with the same RAII guard that scopes
`current_origin` and `current_enrollment_depth`, and treat an enrollment made while it is live as a nested
re-park. `ResidualOrigin` itself cannot serve — it is a parameter of
`fire_prepartitioned_reactions_with_sequences` in `scripting-core`, and the control handler never sees it. Depth bounds causal chains — self-loops and mutual recursion — where the concurrency cap
cannot, because a one-at-a-time loop never raises the instance count. Both mirror `MAX_BATCH_DISPATCH_HOPS`
in `crates/scripting-core/src/reaction_dispatch.rs`: bound the runaway, warn, keep the level running. Note
for the executor that `fire_prepartitioned_reactions_with_sequences` carries a
`#[cfg(debug_assertions)]` assertion against `is_trigger_consequential_primitive` (13 names) which diverges
from the binder's `CONSEQUENTIAL_PRIMITIVES` (15 names, omitting `grantHealth`/`grantAmmo`) — it is not a
full partition check and must not be read as one. Unit tests: O8, O9, O10, O13, O19b, O20, O21, O22, O26,
O26b, O27, O28b, O33, O37, O37b, O38, O40, O45, O47, O48, O49, O50, O62, O64, O65, plus a delayed consequential
landing host-only. Glosses for the rows with no acceptance-criterion restatement: O13 (a resumed
presentation never precedes an earlier one from the same body), O21 (a landed `moverStart` shows first
motion on the next frame's first tick), O22 (a despawned target is skipped with a warn; the rest of the tail
runs), O26 (a landing executes at the drain, after every tick's `evaluate_slot_accumulators`), O26b (a
resumed step reads accumulated slots at their end-of-frame value; a landed write rebases the accumulator),
O27 (landing and residual follow-ups are two batches with independent 256-hop budgets), O37b (a `fire` name
dispatches after the body's presentation, so authored order between them is not preserved), O45 (a landed
spawn is picked up by the next frame's mesh-clip and light sweeps).
Depends on Task 1 for the control arm, the address channel, and the scheduler.

### Task 4: Install validation and Exit-edge derivation

Implement both read-only validation passes. Pass A (V1, V4a, V6) runs where
`slot_accumulator_bindings.rebuild` sits in `install_world_cpu`; Pass B (V2, V3, V4b, V5) runs **after**
`install_manifest_events`, because a V3 check beside Pass A would drop the consumer's own manifest-bound
reveal (O36). A rejected reaction is replaced in place with an inert `Sequence(vec![])`, never removed, so
no later pass or index observes a shifted vector; mutate through `script_ctx.data_registry`
(`Rc<RefCell<DataRegistry>>`), borrowed mutably at each install slot. V1: reject `durationMs` that is zero,
negative, NaN, non-finite, or overflows `u32` ticks — `error!` and drop the reaction, never a silent 1-tick
wait (`NaN as u32` → 0 → `.max(1)`) or a `u32::MAX` countdown. The overflow bound needs the conversion rule
even though Pass A does not convert: `ticks = max(1, ceil(durationMs * 1000 / 16_667))` against
`TICK_DURATION` (`crates/postretro/src/frame_timing.rs`, `Duration::from_micros(16_667)`). Pass B cannot read reaction identity from
`TriggerBindingTable` — `TriggerBinding` holds anonymous `commands` and a `residual` handle, `bind_event`
discards its `matched` Vec after partitioning, and `bound_edges` is a
`HashSet<(EntityId, TriggerEventEdge)>`. Read provenance instead from the sources the binder read:
`TriggerVolumeComponent.on_fire` / `on_exit` across `EntityRegistry` triggers, plus
`data_registry.trigger_events` (tag → trigger ids × `descriptor.fire` names). V2: reject an `interruptible`
wait whose reaction is Enter-bound to a trigger whose `TriggerVolumeComponent.fire_mode`
(`crates/entities/src/components/trigger_volume.rs`) is `TriggerFireMode::Once` — read the component from
the `EntityRegistry`, not the level-format `TriggerVolumeRecord.fire_mode`, which is a `u8` and out of scope
at binding time. The latch is spent on first fire (`update_after_fire` sets `trigger.latched = true`;
`evaluate_trigger_activation` rejects `Once if latched`, both in `crates/postretro/src/trigger_system.rs`),
so a cancel would end the set-piece permanently. V3: reject an `interruptible` wait in a reaction with no
trigger-Enter binding — the flag has no cancel source. A reaction that is both Enter-bound and a `fire`
target still enrolls a sourceless instance on the `fire` path; that instance treats its interruptible waits
as non-interruptible and warns once at enrollment. V4a (Pass A): reject a post-wait step carrying a
`SequenceTarget::Activators`/`FiredTrigger` sentinel. That clause reads only the `DataRegistry` and belongs
in Pass A; `BoundTarget::Activators`/`FiredTrigger` resolve against `TriggerFireContext`
(`BoundTarget::resolve`, `crates/postretro/src/trigger_commands.rs`) and none survive a wait. V4b (Pass B):
reject a post-wait step whose bound program reads a seeded dispatch input, or any `fire` step at any
position whose target's bound program reads one. This clause **cannot** run in Pass A — the predicate is
over a `BoundProgram`, and none is constructed until `build_trigger_bindings`, which runs after Pass A's
slot. Implement only the `fire`-target half. A post-wait *step* needs no check: the trigger binder's sole
`bind(...)` call produces a `BoundStoreValue::Ir` for `setState`, and `bind_sequence_step` refuses
`setState` outright, so a sequence step never yields a `BoundProgram` in any binder and there is nothing
to walk. A `fire` target can be exactly such a system-targeted `setState` `Primitive`, which is where
`@rising` lives, so read it from `SystemReactionIrBindings`
(`crates/postretro/src/scripting/systems/system_reactions.rs`). Its `SystemSetStateBinding` holds `slot`,
`value`, `program`, and `required_dispatch_inputs` but no reaction identity, and `bindings` is private
with no accessor — add the reaction name to that private struct plus a `pub(crate)` accessor returning
`(name, required_dispatch_inputs)`, and read that precomputed field rather than re-walking
`BoundProgram.root`. A reaction bound
by neither binder has no bound program and no seeded input to read; it is out of V4b's reach by
construction, so V4b's acceptance criterion is scoped to bound reactions. `resolve_input` maps a name to a handle at bind time and is
not a query against a bound program, and `BoundProgram` exposes only `root`, `root_type`, and `output`
(`crates/foundation/src/ir/bind.rs`) — so the walk is over `root`, and a name list is the failure mode these
rows exist to avoid. Together V4a and V4b are what make a resumed step's evaluation scope interchangeable
with the binder's. V5 is a derivation, not a
rejection: when an Enter-bound reaction contains an `interruptible` wait, insert
`(trigger, TriggerEventEdge::Exit)` into `bound_edges` through a **new** `bind_edge_only` method on
`TriggerBindingTable`. `bind_event` cannot be reused — it returns early on
`commands.is_empty() && steps.is_empty()`, before the `append_binding` call that is the only existing
inserter, and `bound_edges` is private with a read-only accessor. The insert makes the Exit arm of
`run_authoritative_tick_with_dispatch` stop `continue`-ing past the edge — without this the edge is
silently consumed by `paired_enters.remove` and no cancel source exists. No `bind_edge_only` is needed for
the Enter edge: Task 3's amended binder routes the wait and its tail into the residual, so `steps` is
non-empty even for `[wait(N), moverStart]` and `bind_event` does not take its empty-work early return (O46).
Also add the `paired_enters` accessor Task 5's enrollment check needs — the set is private on
`TriggerSystem` with no reader today, and the check runs from the frame-end drain, which holds the session
and can borrow the system. V6:
warn and drop a `fire` step naming an absent reaction, keeping the rest of the reaction. Both passes must
also be reachable from Task 3's staged-commit re-run. Unit tests: one per row, O36 (V2/V3/V5 run after `install_manifest_events`), O46 (a body whose first step
is the wait still binds its Enter edge), O29 (a malformed `durationMs` drops the reaction and the level
continues), plus the V5
end-to-end that cancellation works with no authored `on_exit` KVP. Depends on Task 1.

### Task 5: Interruptible cancellation, origin, and re-fire

Cancel parked instances from paired Exit edges and define re-fire. Add the scoped **origin guard**: the
scheduler carries `current_origin: Option<(EntityId, PlayerId)>` set by an RAII guard held across the
trigger residual drain in `main.rs` — and **only** that, released before the deferred batch runs. Scoping it
wider is the failure mode to avoid: one batch carries follow-up names seeded by several residuals and its
hops chain inside `scripting-core` with no `postretro` code between them, so a guard spanning the batch
would hand one residual's `(trigger, player)` to another residual's reaction (O54). Every batch-seeded
enrollment is therefore sourceless, which is the case V3's demotion rule already covers. Enrollment of an
interruptible instance additionally verifies its origin's paired enter is still standing — a player who
entered and left within one frame produces a cancel before anything is parked, and without the check the
instance parks uncancellable and the beat fires with nobody on the plate (O52). Cancels apply to the landing
queue as well as to parked instances, so an Exit between expiry and the drain still wins (O53). Instance identity is `(InstanceKey, InstanceId)`, where `InstanceKey` is
`(reaction name, body ordinal, Option<(EntityId, PlayerId)>)`. Never key on a registry position:
`recompose_active_sets` re-derives that ordering (globals filtered by level tag, then locals appended) and a
position key silently retargets across a reload. The **body ordinal** is load-bearing and comes from the
enrolling dispatch, never re-derived: two same-named bodies have identical address and origin, so a
two-part key makes each one's re-fire cancel the other and only the last-dispatched body survives (O56,
O58). A resumed tail carries all three components from its own instance — the landing drain runs outside
the origin guard and has no `matched` loop, so re-deriving would make a nested wait sourceless and give
every body ordinal 0 (O57). **Change `TickEvents.trigger_residuals` from `Vec<TriggerResidualHandle>` to
`Vec<(TriggerResidualHandle, EntityId, PlayerId)>`**, fed from both per-edge dispatch closures in
`crates/postretro/src/sim/mod.rs`, which have `event.fire.trigger` and `event.fire.player` in scope and
already clone `event` beside the push under `#[cfg(test)]`. Today they push `trigger_residuals.push(handle)`
and the `main.rs` drain iterates opaque handles with no player available, so two players entering one plate
push the same handle twice and collapse into one instance. The drain sets `current_origin` per iteration
from the tuple. The retype also breaks an index-style test caller in `crates/postretro/src/sim/mod.rs`
(`bindings.residual(events.trigger_residuals[0])`) — compiler-caught, but name it so it is not a surprise. Also surface the tick's Exit fires into `TickEvents`: production
discards the returned `TriggerFireReport` (both dispatch arms in `simulate_tick_with_presentation_aim` bind
`let _report = …`), and the existing `trigger_fires` field is `#[cfg(test)]`, so add a production
(non-`cfg(test)`) field `trigger_exit_fires`, read in the `RedrawRequested` arm before the per-tick
`evaluate` call, fed from **both** per-edge dispatch closures, and mirror it into `RecordedTick`
via `SimHarness::record` (`crates/postretro/src/sim/determinism_tests.rs`), which projects the real
`TickEvents` rather than cloning the struct. Identity comes from `TriggerEvent.fire` (a
`TriggerEventFire { trigger, player, event_name }`) filtered by `TriggerEvent.edge == Exit`;
`TriggerFireReport.fires` is `Vec<TriggerEvent>`. In `ReactionScheduler::evaluate`, apply cancels **before**
advancing countdowns so an Exit on the landing tick wins (O4). With multiple waits, the wait currently
parked at governs **both** Exit-cancel and re-fire (O17, O18): parked at an interruptible wait, a re-fire is
accepted and restarts from the top of the body; parked at a non-interruptible one, it is ignored exactly as
O7. A restart does not revert effects already applied (O18b). Apply a re-fire's same-key cancel at
enrollment, before the cap test, so a re-enrollment never fails against the cap and never strands the
pre-wait work (O19). Plumb the Exit-fire set into `SimHarness`, which constructs a `TriggerTickContext` and
is the only harness that can host these tests — `run_headless_inner` passes `None` for `trigger_context`
with the comment that "triggers stay inert," so no `TriggerFireReport` is ever produced there and plumbing a
field into it yields an always-empty vec. Unit tests: O3, O4, O5, O6, O7, O11, O17, O18, O18b, O19, O52, O53, O54, O55, O56, O57, O58, O59, O60, O63,
plus two rows with no acceptance-criterion restatement: O16 (one reaction bound to two triggers yields two
instances; each trigger's Exit cancels only its own) and O23 (a player's death or disconnect fires the
paired Exit and cancels, identically to walking off). Depends on Task 4 for
derived Exit edges and Task 1 for enrollment.

### Task 6: Consumer — closet-reveal timed reveal

Author the closet-reveal fixture as a timed, interruptible reveal. In
`content/dev/maps/closet-reveal.map`: add `"_tags" "closet_alarm"` to the `light_spot`, and change the
`closet_reveal_plate` trigger's `"fire_mode"` from `"once"` to `"multiple"` (the `fire_mode` match in
`crates/level-compiler/src/trigger_volumes.rs` maps `"once" | "0" => 0` and `"multiple" | "1" => 1`, and
`bail!`s on any other spelling; the plate's existing `"rearm_ms" "0"` means `update_after_fire` re-arms
immediately, which is what makes re-fire reachable) — required for interruptibility per V2. It does
**not** exercise re-fire-while-parked: the plate is `activation touch` and the Enter gate requires the player
to have left first, which on an interruptible wait fires the paired Exit and cancels. O6 and O7 stay with
Task 5's units; this fixture proves only that the plate can be re-entered to run the beat again. Rewrite `content/dev/scripts/closet-reveal.ts` so the plate's `enter`
fires one sequence whose first step **dispatches** an alarm reaction rather than writing state inline: a
`setState` cannot be a sequence step, because `bind_sequence_step` refuses it outright ("setState is
system-targeted and cannot carry an entity target; not binding") and no `register_sequenced_*` call
registers it, so a `setState` step would be dropped whole by `sequence_primitives_are_valid` at
`setupLevel`. **Remove the existing `closet.openDoor` reaction and drop it from `triggerEvents`** — it binds a
tag-targeted `moverStart` on the plate's enter edge today, so leaving it there opens the door immediately
while every unit test still passes. Bind only `closet.timedReveal` on that edge. Keep the alarm as its own
`defineReaction` holding a `PrimitiveDescriptor` state write —
the shape `crates/postretro/src/netcode/trigger_state_channel_harness_test.rs` exercises — and reach it with
`fire(raiseAlarm)`. That write targets a slot declared `network: "shared"` in the fixture's store
(`StoreSlotSchema`, `crates/scripting-core/src/typedef/templates/sdk_lib.d.ts`; `sharedGlobal` is the
engine-side `ReplicationScope::SharedGlobal` spelling, not the authoring one). **Register that store through
the mod manifest, not the level manifest**: `LevelManifest` has no `stores` key, so stores reach the engine
only via `defineMod({ stores: [...] })`. Put the `defineStore` handle in its own module —
`content/dev/scripts/closet-store.ts`, exporting only the handle — imported by both
`content/dev/scripts/closet-reveal.ts` and `content/dev/start-script.ts`, whose `stores: [...]` array
registers it. That is what the shipped `run-counter.ts` precedent actually models: a store-only module.
Do not import the level script itself into the mod bundle, which would evaluate its reactions in both
bundles — without it the slot is never
created, `network: "shared"` never becomes `ReplicationScope::SharedGlobal`, and the co-op criterion cannot
pass. That slot is what each client's crossing
watcher turns into a client-local `setLightAnimation` on `closet_alarm`. **Author that crossing reaction and
its declaration in the fixture** — required work, not background: the co-op acceptance criterion asserts the
client's local light reaction fires. Import `updateState` from `"postretro/ui"`, not the root; it is
declared in that module and `sdk/lib/index.ts` does not re-export it. E18-A shipped that path end-to-end
including late-join (`plans/done/E18--trigger-event-fanout`); a host-local `setLightAnimation` step would
leave remote players watching a door open with no cue. A reddening alarm is persistent state, not a
transient sting, so it belongs on the channel built for state rather than the one E18-A documents as lossy
for edges. The light is static, and `LightAnimation.color` is accepted on authored static lights
(`validate_and_normalize`, `crates/lighting/src/script_primitives.rs`, whose `_target_is_dynamic` parameter
is unused), so the client-local reaction may animate intensity or colour. Then
`wait(N, { interruptible: true })`, then `moverStart` on the `closet_door` mover resolved by
`world.query({ component: "kinematic_mover", tag: "closet_door" })`, then `fire(releaseCloset)` dispatching
the existing tag-targeted enemy release. The release must be *dispatched*, not inlined: E18-C pins that
tag-targeted primitives ride the Primitive path and never a `sequence`, and
`BoundTriggerCommand::UpdateEnemyState` requires `BoundTarget::Tag` while `bind_sequence_step` maps a
`SequenceTarget::Entity` to `BoundTarget::Entity`. Keep `releaseCloset` as its own `defineReaction` so
`fire()` takes its handle. Delaying it is load-bearing, not cosmetic: the aggro gate is the containment
(E18-C), so releasing at the enter edge would let enemies aggro through a still-closed door for the whole
beat. Author the fixture so no opposing landing shares a frame with the alarm write, since crossings are
frame-sampled (O44). Editing the map requires a `prl-build` pass before the fixture check can run. Fixture check backing the consumer AC: alarm at press, door and enemies only after the
wait, leaving the plate mid-wait leaves the door closed and enemies contained, and the plate re-fires.
Depends on Tasks 2, 3, 4, 5.

### Task 7: Ordering, determinism, and replication coverage

Author the cross-cutting headless tests for Ordering rows not covered by a single task's units — O15 (stall
clamp stretching the wall-clock delay), O24 and O43 (client enrolls nothing, and says so), O25
(landing-order determinism across two identical runs), O28 (self-retriggering wait bounded by depth), O30
(Luau-side V4b rejection of a scoped `fire`), O32 (UI and crossing fires), O39 (suspend-path clear), O44
(frame-sampled crossing) — plus the two-endpoint replication checks: that a delayed `fire` step dispatching a
system-targeted `setState` reaction reaches a client with no client-side scheduler — not `moverStart`, whose
phase rides its own `kinematic_mover` component record rather than `state_records` — and that the consumer's
alarm crossing fires client-side (O42), using
the harness patterns in `netcode/state_slot_loss_harness_test.rs` and
`netcode/trigger_state_channel_harness_test.rs`. Several of these rows are **frame**-shaped, and neither
N-tick driver has a frame: `run_headless_inner` and `SimHarness` both run one `simulate_tick` per loop
iteration with no frame-end drain — `run_headless_inner` has no `pending_trigger_residuals` and never calls
`fire_prepartitioned_reactions_with_sequences`, and `SimHarness::record` copies `trigger_residuals` into
`RecordedTick` without resolving them. Task 1 adds `SimHarness::frame(&mut self, commands: &[RecordedCommand])` — one tick per command, then the
frame-end drain — so route every frame-shaped row through it here rather than building it; `run_headless_inner` additionally has no trigger context and no net
endpoint, so it owns none of the cancel, O24, or O39 rows. Name which harness owns which row. Seed a fixed
`1.0/60.0` DT. Land new test modules as new files rather than growing `sim/determinism_tests.rs` (3652
lines). Depends on Tasks 3 and 5.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies descriptor → deserialize → dispatch arm →
enrollment → landing frame before fan-out. It owns the `install_world_cpu` reach for the scheduler and the
`WorldInstallHandles` field, so Task 3's registrations have somewhere to land.
**Phase 2 (concurrent):** Task 2 (SDK builders — needs only the settled wire shapes), Task 3 (resume
execution, lifecycle, cycle breakers).
**Phase 3 (sequential):** Task 4 — shares `install_world_cpu` and the staged-commit block with Task 3.
**Phase 4 (sequential):** Task 5 — consumes Task 4's derived Exit edges and Task 1's enrollment.
**Phase 5 (concurrent):** Task 6 (consumer), Task 7 (ordering/determinism/replication tests).

## Boundary inventory

| Name | Rust (internal) | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| wait step | `SequenceTarget::Wait` (payload-free, `Copy`) | `{ "id": "@wait", "primitive": "wait", "args": {…} }` | `wait(durationMs, opts)` → `SequenceStep[]` | `wait(durationMs, opts)` | n/a |
| fire step | `SequenceTarget::Fire` (payload-free, `Copy`) | `{ "id": "@fire", "primitive": "fire", "args": { "event": … } }` | `fire(reaction \| name)` → `SequenceStep[]` | `fire(reaction \| name)` | n/a |
| wait sentinel | control target variant | `"@wait"` | `"@wait"` | `"@wait"` | n/a |
| fire sentinel | control target variant | `"@fire"` | `"@fire"` | `"@fire"` | n/a |
| delay | whole tick count (install-time integer conversion) | `"durationMs"` (number, ms) | `durationMs` | `durationMs` | n/a |
| interruptible | `bool` | `"interruptible"` (bool, default false) | `interruptible` | `interruptible` | n/a |
| fire target | reaction identity string | `"event"` (string) | `Reaction<{}> \| string` | `Reaction \| string` | n/a |

Milliseconds cross the wire; ticks are internal. Conversion is integer micros against `TICK_DURATION`
(`crates/postretro/src/frame_timing.rs`, `Duration::from_micros(16_667)`):
`ticks = max(1, ceil(durationMs * 1000 / 16_667))`. Use the integer form because `TICK_DURATION` is the
authoritative constant, not because a float rule disagrees — `ceil(ms / (1000/60))` yields the same tick
count at every duration from 1ms to 5000ms in both `f32` and `f64`. Existing sentinels
`"@activators"`/`"@trigger"` are the naming precedent; `fire()` resolves a handle to its name exactly as
`onTriggerEvent` does.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| No dispatch path runs past a `wait` in its own drain | Task 1 (control arm enrolls the remainder and `break`s, below every named-dispatch entry point) | an enrollment sink threaded through signatures would miss hops inside `dispatch_deferred_named_events_with_sequences`, whose loop `scripting-core` owns | O34, O32, O35 |
| Nothing the scheduler depends on is stored in a reaction body or keyed by its position | Task 1 (tail is `&steps[i+1..]` read live), Task 5 (key on reaction name, ordinal, and origin) | `recompose_active_sets` rebuilds `DataRegistry.reactions` from retained originals at thirteen sites across four files, erasing body mutations and reordering positions | O40, O41 |
| The sim tick stays VM-free | Task 1 (`dispatch_sequence` is never called inside `simulate_tick`; in-tick trigger work runs bound commands and defers steps as a residual handle) | siting enrollment or landing inside the tick loop would add a VM call to the tick | VM-free review gate (the `ResidualOrigin::TriggerBinding` assertion), pre-wait `moverStart` AC |
| An instance never advances in the frame that enrolled it | Task 1 (monotonic frame-counter stamp; `evaluate` skips the current frame) | being outside the tick loop is not enough — `fire_focused_button_activation` dispatches *before* the loop in the same redraw, so a pre-loop enrollment would advance immediately; `tick_index` cannot serve as the stamp because it resets per frame | O1, O2, O12, O31, O51, O25 |
| A trigger-bound wait reaches the scheduler instead of being deleted at install | Task 3 (binder arm stops at the first `Wait`; wait and tail go to the residual unfiltered) | `partition_direct_reaction` otherwise hoists post-wait consequential steps in-tick by class and drops the wait and every `fire` as a non-`Entity` sentinel target | O49, O50, Consumer AC |
| Per-enrollment provenance is transported, never re-derived | Task 1 (address and ordinal as `dispatch_sequence` parameters), Task 3 (both ride on `PrepartitionedReactionStep::Descriptor`; instances store all three and the resume passes them back), Task 5 (`TickEvents.trigger_residuals` carries trigger and player; guard set per drain iteration) | a residual is N reactions' product, so a single address on `TriggerResidual` is wrong; the drain sees only opaque handles, so two players on one plate collapse to one instance; the resume path has no `matched` loop, so a re-derived ordinal is 0 for every body | O54, O55, O56, O57, O58, O59, O60 |
| Delayed consequential effects are host-authoritative and replicated, never client-re-simmed | Task 1 (host-only guard at the scheduler entry point, not at a call site) | named fires run on clients too, so a call-site guard leaks | Co-op replication AC, O24, O43 |
| An instance lands exactly once, or is cancelled exactly once — never both | Task 1 (enroll/resume), Task 5 (cancel before advance) | cancel must precede countdown advance in a tick (O4); a re-fire must not leave two instances (O6, O19) | O3, O4, O6, O19 |
| A resumed tail executes where a trigger residual executes, with no new machinery | Task 3 (`Descriptor(Sequence(tail))` through the `pub` `fire_prepartitioned_reactions_with_sequences`) | appending to `pending_trigger_residuals` cannot work — it holds bare indices into `TriggerBindingTable::residuals` | O13, O21, O26, O33 |
| Level teardown, suspend, and hot reload all drop pending instances | Task 3 (clear in `clear_surface_lifetime_level_state` and in the staged-commit block) | that function clears field by field, so an unregistered structure survives; a stored tail outlives the body it was copied from | O9, O20, O38, O39, O40 |
| An `interruptible` wait always has a live cancel source, or is inert and warned | Task 4 (V2, V3 reject; V5 derives the Exit edge via `bind_edge_only`) | the Exit arm `continue`s when unbound; `bind_event` early-returns before the only existing inserter; a sourceless `fire`-path instance has no Exit at all | V-row ACs, O36, Consumer AC |
| No resumed step reads fire-time context | Task 4 (V4a/V4b reject sentinels, any `Dispatch(_)` input reader, and scoped `fire` targets at any position), Task 2 (`Reaction<{}>` gate) | the binder otherwise pre-partitions `@activators`/`@trigger` targets and seeded-input readers into a tail; the type gate misses the `\| string` overload and all Luau | O30, V4b AC |

## Script syntax examples

```typescript
// Proposed design — closet-reveal timed reveal (Task 6).
import { defineReaction, defineStore, enemies, fire, onTriggerEvent, wait, world } from "postretro";
// `updateState` is declared in the "postretro/ui" module, not the root — it is not re-exported by
// sdk/lib/index.ts.
import { updateState } from "postretro/ui";

// Replicates to every client; each one's crossing watcher lights the alarm locally.
// defineStore takes a flat slot map; the default-value key is `default`, not `initial`.
const store = defineStore("encounter", { alarm: { type: "number", default: 0, network: "shared" } });

// System-targeted state write: a Primitive reaction, dispatched — never a sequence step.
const raiseAlarm = defineReaction("closet.raiseAlarm", updateState(store.alarm, 1));

// Tag-targeted, so it stays its own reaction on the Primitive path (E18-C).
const releaseCloset = defineReaction(
  "closet.releaseCloset",
  enemies({ tag: "closet_enemies" }).update({ aggro: true }),
);

export function setupLevel() {
  const door = world.query({ component: "kinematic_mover", tag: "closet_door" });

  // One authored beat: raise the alarm now, hold, then slam and release together.
  // Stepping off the plate during the hold cancels both. The alarm rides the
  // shared-network slot so every player sees it, not just the host.
  const reveal = defineReaction("closet.timedReveal", {
    sequence: [
      ...fire(raiseAlarm),                    // replicates; clients light it locally
      ...wait(800, { interruptible: true }),  // enrolls the remainder, stops here
      ...door.flatMap((m) => m.start()),      // resumes ~48 ticks later
      ...fire(releaseCloset),                 // dispatches the tag-targeted release
    ],
  });

  return {
    // alarmSlot is declared `network: "shared"` in the fixture's store, and a
    // client-local crossing reaction turns it into setLightAnimation on closet_alarm.
    reactions: [reveal, raiseAlarm, releaseCloset],
    triggerEvents: [
      // No "exit" registration needed — V5 derives the Exit edge from the interruptible wait.
      onTriggerEvent({ tag: "closet_reveal_plate" }, "enter", [reveal]),
    ],
  };
}
```

## Owner decisions

Four ship-time choices that no source forecloses — surfaced rather than buried.

- **Capturing the chained names reactivates `onComplete` at six call sites.** A `fire` step only reaches
  `dispatch_deferred_named_events_with_sequences` if its caller consumes the returned vec, and today every
  production caller of `fire_named_event_with_sequences` discards it. Making `fire` work at those sites
  means capturing the vec, and the vec also carries `PrimitiveDescriptor.on_complete` names — so an
  `onComplete` chain authored on a `levelLoad`, UI, crossing, death, or mover-sound reaction begins running
  where it is silently dropped today. This is a latent-defect fix rather than a regression, and the
  alternative — routing `fire` names on a channel separate from `chained` purely to keep the existing
  silence — buys nothing and costs a parallel return path. Default chosen: capture at all six, and cover
  the reactivation with O48 so it is tested rather than discovered. Flag if any shipped content depends on
  an `onComplete` chain staying inert.

- **Waits advance while the pause menu is open.** No engine-wide pause concept exists to inherit —
  `renderer.freeze_time()` gates the animation sample clock only, and its call site is
  `#[cfg(feature = "dev-tools")]`, with `let frozen = false` in the non-dev build. No engine gate freezes
  the fixed tick: `main.rs` gates only input on `ui_captures_gameplay`, while the tick loop and
  `evaluate_slot_accumulators` run ungated. The scheduler matches shipped accumulator behavior. Freezing on
  pause would be new cross-cutting scope this spec does not own. Default chosen: advance. Flag if a
  set-piece needs pause-freeze.
- **A wall-clock delay stretches under stalls.** `MAX_ACCUMULATOR` clamps the tick accumulator, so a 2s stall
  delivers ≤14 ticks and a `wait(800)` takes 800ms plus the stalled time (O15). Correcting this would mean
  wall-clock delays, which co-op determinism rules out. Default chosen: tick-counted, stretch accepted.
- **A client-local reaction containing a wait loses its tail.** The scheduler is host-only, but named fires
  — `levelLoad` at install, UI presses, and the client-local crossing reactions E18-A's atmosphere channel
  depends on — run on connected clients too. Nothing forecloses an author putting a wait in one: no
  validation rejects it, no type prevents it, and the call site is reached. This is a defect the spec ships,
  not an inherited footnote: E18-A's limit is that a *host-fired* transient does not reach clients, which is
  a different case. Default chosen: refuse enrollment at the host-only guard and `warn!` once per reaction
  naming it (O43), so the loss is visible. The alternative — a client-side scheduler restricted to
  presentation-only tails — is a named additive follow-up. Flag if client-side timed presentation is wanted
  in v1.
