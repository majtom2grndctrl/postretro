// E18 Task 7: cross-cutting ReactionScheduler ordering/lifecycle coverage that
// does not need a tick-driven harness. Every test here drives `ReactionScheduler`
// (Task 1/3/5, `reaction_scheduler.rs`) directly against a hand-built
// `DataRegistry`/`ScriptCtx`, exactly like that module's own unit tests. It is a
// separate file to keep these harness-free ordering rows apart from the
// tick-driven ones, which need a different fixture.
//
// See: context/plans/in-progress/E18--timed-reaction-steps/index.md — Ordering
// scenarios, Task 3 (deferred frame-shaped rows), Task 7.

#![cfg(test)]

use std::cell::RefCell;
use std::rc::Rc;

use postretro_entities::{
    DataRegistry, EntityId, NamedReaction, PrimitiveDescriptor, ReactionDescriptor, ScriptCtx,
    SequenceStep, SequenceTarget, Transform,
};
use postretro_scripting_core::reaction_dispatch::{
    ResidualOrigin, fire_named_event_with_sequences, fire_prepartitioned_reactions_with_sequences,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_test_log_capture::LogCapture;

use super::reaction_scheduler::{
    MAX_PENDING_REACTION_INSTANCES, MAX_REACTION_CHAIN_DEPTH, ReactionScheduler,
    register_reaction_control_primitives,
};
use crate::trigger_system::PlayerId;

/// Shared fixture: a `ScriptCtx` plus the four dispatch registries the resume
/// path needs, and an enabled scheduler with `wait`/`fire` registered. Mirrors
/// the wiring `SimHarness::new` and `session/mod.rs` both do, minus anything
/// tick/frame-specific.
struct Fixture {
    ctx: ScriptCtx,
    scheduler: ReactionScheduler,
    data: DataRegistry,
    sequence_registry: SequencedPrimitiveRegistry,
    reaction_registry: ReactionPrimitiveRegistry,
    system_registry: SystemReactionRegistry,
    /// Ordered call log shared by every `note`-style test primitive registered
    /// below. Cheap, order-observable proof of "did this step run, and when
    /// relative to other steps" — the same technique `SimHarness`'s `note_log`
    /// uses.
    log: Rc<RefCell<Vec<String>>>,
}

impl Fixture {
    fn new() -> Self {
        let ctx = ScriptCtx::new();
        let scheduler = ReactionScheduler::default();
        scheduler.set_enabled(true);
        let mut sequence_registry = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut sequence_registry, scheduler.clone());
        let log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        {
            let log = log.clone();
            sequence_registry.register("note", move |_id, args| {
                if let Some(label) = args.get("label").and_then(serde_json::Value::as_str) {
                    log.borrow_mut().push(label.to_string());
                }
                Ok(())
            });
        }
        let mut reaction_registry = ReactionPrimitiveRegistry::new();
        {
            let log = log.clone();
            reaction_registry.register_tagged("note", move |_reg, _tag, _targets, args| {
                if let Some(label) = args.get("label").and_then(serde_json::Value::as_str) {
                    log.borrow_mut().push(label.to_string());
                }
                Ok(())
            });
        }
        let system_registry = SystemReactionRegistry::new();
        Self {
            ctx,
            scheduler,
            data: DataRegistry::new(),
            sequence_registry,
            reaction_registry,
            system_registry,
            log,
        }
    }

    fn install(&mut self, reactions: Vec<NamedReaction>) {
        self.ctx
            .data_registry
            .borrow_mut()
            .populate_level(reactions.clone(), Vec::new(), &[]);
        // The scheduler's own dispatch calls take a caller-owned `DataRegistry`
        // (`&self.data`), mirroring how `fire_prepartitioned_reactions_with_sequences`
        // is fed `dispatch_data` in `SimHarness`/`session/mod.rs` rather than
        // reading through `ctx.data_registry` directly.
        self.data.populate_level(reactions, Vec::new(), &[]);
    }

    fn spawn_entity(&self) -> EntityId {
        self.ctx.registry.borrow_mut().spawn(Transform::default())
    }

    fn note_step(&self, id: EntityId, label: &str) -> SequenceStep {
        SequenceStep {
            id: SequenceTarget::Entity(id),
            primitive: "note".to_string(),
            args: serde_json::json!({ "label": label }),
        }
    }

    fn fire_step(&self, event: &str) -> SequenceStep {
        SequenceStep {
            id: SequenceTarget::Fire,
            primitive: "fire".to_string(),
            args: serde_json::json!({ "event": event }),
        }
    }

    fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    /// Enroll a wait directly (bypassing `dispatch_sequence`/the control-handler
    /// path), matching how `reaction_scheduler.rs`'s own unit tests drive the
    /// scheduler. `ticks` is already the whole-tick countdown.
    fn enroll(
        &self,
        address: &str,
        ordinal: usize,
        origin: Option<(EntityId, PlayerId)>,
        tail: Vec<SequenceStep>,
        ticks: u32,
        interruptible: bool,
    ) {
        self.scheduler
            .enroll(address, ordinal, origin, tail, ticks, interruptible);
    }

    /// Advance one tick with no trigger Exit fires and one frame.
    fn tick_and_frame(&self) {
        self.scheduler.begin_frame();
        self.scheduler.evaluate(&[]);
    }

    fn drain(&self) {
        self.scheduler.drain_landings(
            &self.data,
            &self.sequence_registry,
            &self.reaction_registry,
            &self.system_registry,
            &self.ctx,
        );
    }
}

// ---------------------------------------------------------------------------
// O20: a lifecycle-shaped step (`loadLevel`) in a resumed tail runs at the
// drain, exactly where a trigger residual's own step would; teardown at that
// point drops every OTHER still-parked instance.
//
// GAP: `loadLevel` here is a `note`-recording stand-in, not the real primitive
// — this harness has no boot/window loop to observe an actual level swap. What
// is proven: the resumed step executes during `drain_landings` (the same call
// a trigger residual's steps run through), and a subsequent
// `ReactionScheduler::clear()` (the production hook `clear_surface_lifetime_level_state`
// calls) drops sibling parked instances.
// ---------------------------------------------------------------------------
#[test]
fn resumed_lifecycle_step_runs_at_drain_and_teardown_drops_siblings() {
    let mut fx = Fixture::new();
    let target = fx.spawn_entity();
    fx.install(vec![]);

    // One instance about to land with a `loadLevel`-shaped tail.
    fx.enroll(
        "reveal",
        0,
        None,
        vec![fx.note_step(target, "loadLevel")],
        1,
        false,
    );
    // A sibling instance parked much longer — still pending when teardown hits.
    fx.enroll(
        "sibling",
        0,
        None,
        vec![fx.note_step(target, "siblingTail")],
        1000,
        false,
    );

    fx.tick_and_frame();
    assert!(fx.log().is_empty(), "no step runs before the drain");
    fx.drain();
    assert_eq!(
        fx.log(),
        vec!["loadLevel".to_string()],
        "the resumed tail's step runs at the drain, matching a trigger residual's own step",
    );
    assert_eq!(fx.scheduler.pending_len(), 1, "the sibling is still parked");

    // Teardown: `clear_surface_lifetime_level_state` calls `scheduler.clear()`.
    fx.scheduler.clear();
    assert_eq!(
        fx.scheduler.pending_len(),
        0,
        "teardown after the landing drops the still-parked sibling instance",
    );
}

// ---------------------------------------------------------------------------
// O21: a landed consequential step runs at the drain.
//
// GAP: no `KinematicMoverComponent`/collider is modeled here — this harness has
// none of that machinery. What is proven: a step named `moverStart` in a
// resumed tail executes during `drain_landings`, not during `evaluate`
// (countdown advance) — the same "drain, not tick" offset O21's row states for
// a trigger residual's own `moverStart`.
// ---------------------------------------------------------------------------
#[test]
fn landed_mover_start_shaped_step_runs_at_the_drain_not_during_evaluate() {
    let mut fx = Fixture::new();
    let target = fx.spawn_entity();
    fx.install(vec![]);
    fx.enroll(
        "openDoor",
        0,
        None,
        vec![fx.note_step(target, "moverStart")],
        1,
        false,
    );
    fx.tick_and_frame();
    assert!(
        fx.log().is_empty(),
        "evaluate() only advances the countdown and enqueues the landing; it must not run the step"
    );
    fx.drain();
    assert_eq!(fx.log(), vec!["moverStart".to_string()]);
}

// ---------------------------------------------------------------------------
// O22: a despawned target is skipped with a warn; the rest of the tail runs.
// ---------------------------------------------------------------------------
#[test]
fn despawned_target_is_skipped_with_warn_and_the_rest_of_the_tail_runs() {
    let mut fx = Fixture::new();
    let gone = fx.spawn_entity();
    let survivor = fx.spawn_entity();
    fx.install(vec![]);
    fx.ctx
        .registry
        .borrow_mut()
        .despawn(gone)
        .expect("fixture entity despawns before landing");

    fx.enroll(
        "reveal",
        0,
        None,
        vec![
            fx.note_step(gone, "gone"),
            fx.note_step(survivor, "survivor"),
        ],
        1,
        false,
    );
    fx.tick_and_frame();
    let capture = LogCapture::start();
    fx.drain();
    capture.assert_logged_once(log::Level::Warn, "not found, skipping");
    assert_eq!(
        fx.log(),
        vec!["survivor".to_string()],
        "the despawned step is skipped; the remaining tail step still runs"
    );
}

// ---------------------------------------------------------------------------
// O27 (full): a resumed tail's `fire` step plus the same frame's OTHER landing
// each get their own `dispatch_deferred_named_events_with_sequences` call with
// an independent 256-hop budget — `(K + 1) * 256` for K landings, not one
// shared 256 across the whole frame. Two on_complete chains of 200 hops each
// (well under 256 alone, well over 256 combined) both complete in full when
// they land in the SAME `drain_landings()` call, proving the budgets are
// independent per landing.
// ---------------------------------------------------------------------------
#[test]
fn each_landing_gets_its_own_independent_hop_budget() {
    const CHAIN_LEN: usize = 200;
    let mut fx = Fixture::new();
    let target = fx.spawn_entity();

    fn chain(prefix: &str, len: usize) -> Vec<NamedReaction> {
        (0..len)
            .map(|i| {
                let name = format!("{prefix}{i}");
                let next = (i + 1 < len).then(|| format!("{prefix}{}", i + 1));
                NamedReaction {
                    name,
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "note".to_string(),
                        target: None,
                        tag: Some("chain-tag".to_string()),
                        on_complete: next,
                        args: serde_json::json!({ "label": format!("{prefix}{i}") }),
                    }),
                }
            })
            .collect()
    }
    fx.ctx
        .registry
        .borrow_mut()
        .set_tags(target, vec!["chain-tag".to_string()])
        .expect("fixture entity accepts the chain tag");

    let mut reactions = chain("a", CHAIN_LEN);
    reactions.extend(chain("b", CHAIN_LEN));
    fx.install(reactions);

    fx.enroll("landingA", 0, None, vec![fx.fire_step("a0")], 1, false);
    fx.enroll("landingB", 0, None, vec![fx.fire_step("b0")], 1, false);
    fx.tick_and_frame();
    let capture = LogCapture::start();
    fx.drain();
    capture.assert_not_logged(log::Level::Warn, "aggregate batch cap");

    let log = fx.log();
    let a_count = log.iter().filter(|entry| entry.starts_with('a')).count();
    let b_count = log.iter().filter(|entry| entry.starts_with('b')).count();
    assert_eq!(
        a_count, CHAIN_LEN,
        "landing A's 200-hop chain completes under its own 256 budget"
    );
    assert_eq!(
        b_count, CHAIN_LEN,
        "landing B's 200-hop chain completes under its OWN separate 256 budget \
         (400 combined hops would exceed one shared 256-hop cap)"
    );
}

// ---------------------------------------------------------------------------
// O28: a self-retriggering wait `R = [x, wait(N), fire(R)]` must terminate at
// `MAX_REACTION_CHAIN_DEPTH` with a single warning naming the reaction.
//
// Regression: the scheduler once held one `resume_context` cell across BOTH a
// landing's tail dispatch AND its per-instance deferred dispatch, and classified
// a nested re-park vs a `fire`-seeded child by matching `(address, body_ordinal)`
// against it. A body that `fire()`s its own name re-enters during the deferred
// dispatch with the IDENTICAL key as the resuming instance, so it was misread as
// a nested same-body re-park (depth kept, cap-exempt) rather than a fresh
// fire-seeded child — depth never incremented and the loop ran unbounded. The fix
// split the cell in two, scoping re-park detection (`currently_resuming`) to the
// tail dispatch only while depth attribution (`current_enrollment_depth`) spans
// both, so a self-`fire` child re-entering one phase later is correctly a child.
// ---------------------------------------------------------------------------
#[test]
fn self_retriggering_wait_terminates_at_max_chain_depth() {
    let mut fx = Fixture::new();
    let target = fx.spawn_entity();
    fx.install(vec![NamedReaction {
        name: "loop".to_string(),
        descriptor: ReactionDescriptor::Sequence(vec![
            fx.note_step(target, "x"),
            SequenceStep {
                id: SequenceTarget::Wait,
                primitive: "wait".to_string(),
                args: serde_json::json!({ "durationMs": 1.0, "interruptible": false }),
            },
            fx.fire_step("loop"),
        ]),
    }]);

    // Kick off the chain exactly as a trigger-fired origin would: a direct
    // named fire (sourceless here; the mechanism under test does not depend on
    // origin — see the scheduler's own `effective_origin` docs).
    let chained = fire_named_event_with_sequences(
        "loop",
        &fx.data,
        &fx.sequence_registry,
        &fx.reaction_registry,
        &fx.system_registry,
        &fx.ctx,
        None,
    );
    assert!(
        chained.is_empty(),
        "the wait stops the drain before any fire collects"
    );

    // Advance frames well past MAX_REACTION_CHAIN_DEPTH (256) self-fire cycles.
    // Each cycle is a 1-tick wait: one `tick_and_frame` + `drain` per cycle.
    let capture = LogCapture::start();
    let mut depth_cap_warned = false;
    for _ in 0..(MAX_REACTION_CHAIN_DEPTH as usize + 20) {
        fx.tick_and_frame();
        fx.drain();
        if fx.scheduler.pending_len() == 0 {
            // The chain stopped enrolling a fresh instance: either it was
            // dropped at the depth cap, or something else halted it.
            depth_cap_warned = capture.records().iter().any(|record| {
                record.level == log::Level::Warn && record.message.contains("enrolled-by chain cap")
            });
            break;
        }
    }
    assert!(
        depth_cap_warned,
        "expected the self-fire loop to terminate with exactly one \
         `{MAX_REACTION_CHAIN_DEPTH}-deep enrolled-by chain cap` warning; instead it ran past \
         {} cycles without one, which means depth never incremented across this self-referential \
         chain (see the final report)",
        MAX_REACTION_CHAIN_DEPTH as usize + 20,
    );
    capture.assert_logged_once(log::Level::Warn, "enrolled-by chain cap");
}

// ---------------------------------------------------------------------------
// O39: a suspend-path clear drops parked instances with a `warn!` naming the
// count. `clear()` is the same call `clear_surface_lifetime_level_state` makes
// on both the level-teardown and the suspend path (O38/O39/O40 share the hook;
// Task 3 already asserts the structural drop, this asserts the log contract).
// ---------------------------------------------------------------------------
#[test]
fn suspend_path_clear_warns_with_the_dropped_count() {
    let fx = Fixture::new();
    let target = fx.spawn_entity();
    fx.enroll("a", 0, None, vec![fx.note_step(target, "a")], 50, false);
    fx.enroll("b", 0, None, vec![fx.note_step(target, "b")], 50, false);
    fx.enroll("c", 0, None, vec![fx.note_step(target, "c")], 50, false);
    assert_eq!(fx.scheduler.pending_len(), 3);

    let capture = LogCapture::start();
    fx.scheduler.clear();
    capture.assert_logged_once(
        log::Level::Warn,
        "dropping 3 pending timed-reaction instance",
    );
    assert_eq!(fx.scheduler.pending_len(), 0);
}

// ---------------------------------------------------------------------------
// O43: a client-local reaction containing a wait warns exactly once and drops
// its tail, rather than failing silently. The scheduler's host-only guard is
// driven the same way O24 drives it: `set_enabled(false)` directly, matching
// `auto_close.rs`'s own precedent (no harness constructs a `NetEndpoint`).
// ---------------------------------------------------------------------------
#[test]
fn client_local_reaction_with_wait_warns_once_and_parks_nothing() {
    let fx = Fixture::new();
    fx.scheduler.set_enabled(false);
    let capture = LogCapture::start();
    fx.enroll("crossingPresentation", 0, None, vec![], 10, false);
    capture.assert_logged_once(log::Level::Warn, "wait enrollment refused");
    capture.assert_logged_once(log::Level::Warn, "crossingPresentation");
    assert_eq!(fx.scheduler.pending_len(), 0, "no tail parks client-side");
    fx.tick_and_frame();
    fx.drain();
    assert!(fx.log().is_empty(), "no tail ever runs client-side");
}

// ---------------------------------------------------------------------------
// O45: a landed spawn-shaped step runs at the drain.
//
// GAP: no real `spawnFromSpawner` / mesh-clip-resolve / dynamic-light-sweep
// machinery is modeled here. What is proven is the same drain-phase ordering
// established generically above (O20/O21): the resumed step executes
// during `drain_landings`, matching where a trigger residual's own spawn
// would run today, so the next frame is what picks it up — this harness
// cannot independently verify the clip/light sweep itself.
// ---------------------------------------------------------------------------
#[test]
fn landed_spawn_shaped_step_runs_at_the_drain() {
    let mut fx = Fixture::new();
    let target = fx.spawn_entity();
    fx.install(vec![]);
    fx.enroll(
        "spawnBeat",
        0,
        None,
        vec![fx.note_step(target, "spawnFromSpawner")],
        1,
        false,
    );
    fx.tick_and_frame();
    assert!(fx.log().is_empty());
    fx.drain();
    assert_eq!(fx.log(), vec!["spawnFromSpawner".to_string()]);
}

// ---------------------------------------------------------------------------
// O64 (full): with the cap at capacity, an instance whose OWN landing
// re-parks it at a nested second wait still succeeds, driven through the REAL
// mechanism (`drain_landings`'s `resume_context`, not a hand-simulated
// "already past the first wait" direct enrollment — an earlier draft of this
// test enrolled the second-wait tail directly and wrongly expected cap
// exemption; `enroll` only exempts a re-enrollment made WHILE `resume_context`
// is live, i.e. from inside that instance's own `drain_landings` call).
//
// SCOPED DOWN from the row's full text: whether a *different*, freshly
// residual-dispatched fire can steal a slot an expiring instance freed
// between `evaluate` and `drain_landings` in the same real frame is NOT
// asserted here — this harness calls `evaluate` and `drain_landings`
// directly, one after the other, with no way to interleave a THIRD caller's
// enrollment into that exact gap the way a real frame's residual dispatch
// would. That narrower claim is unverified by this test; see the final report.
// ---------------------------------------------------------------------------
#[test]
fn cap_holds_a_landing_instances_slot_across_the_same_frame_drain() {
    let mut fx = Fixture::new();
    let target = fx.spawn_entity();

    fn two_wait_body(fx: &Fixture, target: EntityId, addr: &str) -> Vec<NamedReaction> {
        vec![NamedReaction {
            name: addr.to_string(),
            descriptor: ReactionDescriptor::Sequence(vec![
                fx.note_step(target, &format!("{addr}:1")),
                SequenceStep {
                    id: SequenceTarget::Wait,
                    primitive: "wait".to_string(),
                    args: serde_json::json!({ "durationMs": 1.0, "interruptible": false }),
                },
                fx.note_step(target, &format!("{addr}:2")),
                SequenceStep {
                    id: SequenceTarget::Wait,
                    primitive: "wait".to_string(),
                    args: serde_json::json!({ "durationMs": 1.0, "interruptible": false }),
                },
                fx.note_step(target, &format!("{addr}:3")),
            ]),
        }]
    }

    let mut reactions = two_wait_body(&fx, target, "multiWaitA");
    reactions.extend(two_wait_body(&fx, target, "multiWaitB"));
    fx.install(reactions);

    // Fire both bodies for real: each runs its first `note`, hits its FIRST
    // wait, and enrolls fresh (depth 0, cap-tested) — this is the legitimate
    // "occupies a slot from enrollment" case.
    for address in ["multiWaitA", "multiWaitB"] {
        let chained = fire_named_event_with_sequences(
            address,
            &fx.data,
            &fx.sequence_registry,
            &fx.reaction_registry,
            &fx.system_registry,
            &fx.ctx,
            None,
        );
        assert!(chained.is_empty());
    }
    assert_eq!(fx.scheduler.pending_len(), 2);

    // Fill the REMAINING slots so the pool sits exactly at capacity with
    // multiWaitA/B occupying two of them.
    for ordinal in 0..(MAX_PENDING_REACTION_INSTANCES - 2) {
        fx.enroll(
            "filler",
            ordinal,
            None,
            vec![fx.note_step(target, "filler")],
            1000,
            false,
        );
    }
    assert_eq!(fx.scheduler.pending_len(), MAX_PENDING_REACTION_INSTANCES);

    // Both expire on this tick (their first wait was a 1-tick countdown) and
    // land. Their OWN resumed tail runs `note(:2)`, then hits its SECOND
    // wait — enrolled from INSIDE `drain_landings`'s `resume_context` for
    // this exact instance, so it is a nested re-park (cap-exempt), not a
    // fresh enrollment.
    fx.tick_and_frame();
    fx.drain();
    assert_eq!(
        fx.log()
            .iter()
            .filter(|s| s.as_str() == "multiWaitA:2")
            .count(),
        1
    );
    assert_eq!(
        fx.log()
            .iter()
            .filter(|s| s.as_str() == "multiWaitB:2")
            .count(),
        1
    );
    assert_eq!(
        fx.scheduler.pending_len(),
        MAX_PENDING_REACTION_INSTANCES,
        "both nested re-parks at the second wait succeeded even though the pool was \
         already at capacity going into this tick — a fresh (non-nested) enrollment at \
         this same capacity is cap-tested and dropped (proven separately, above: \
         `cap_drops_excess_enrollments_and_leaves_parked_instances_untouched`)"
    );

    // Both land again (second wait) and complete their bodies.
    fx.tick_and_frame();
    fx.drain();
    assert_eq!(
        fx.log()
            .iter()
            .filter(|s| s.as_str() == "multiWaitA:3")
            .count(),
        1
    );
    assert_eq!(
        fx.log()
            .iter()
            .filter(|s| s.as_str() == "multiWaitB:3")
            .count(),
        1
    );
}

// ---------------------------------------------------------------------------
// Delayed-consequential-landing, host-only (Task 3 handoff, no O-row number):
// a landed `fire` step dispatching a system-targeted `setState` runs entirely
// host-side through `fire_prepartitioned_reactions_with_sequences` with
// `ResidualOrigin::ResumedTail` — no client machinery is touched by the
// scheduler at all (the client-refusal half is O24/O43, covered separately;
// the wire-level replication half is O42, covered by the two-endpoint harness).
// ---------------------------------------------------------------------------
#[test]
fn delayed_fire_of_a_system_reaction_dispatches_host_only_through_the_resumed_tail_path() {
    let mut fx = Fixture::new();
    // A sourceless system `setState` reaction — V4b only rejects targets that
    // read a seeded dispatch input; a plain write is a legitimate `fire` target.
    fx.install(vec![NamedReaction {
        name: "raiseAlarm".to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: "setState".to_string(),
            target: None,
            tag: None,
            on_complete: None,
            args: serde_json::json!({
                "slot": "puzzle.alarm",
                "value": { "op": "const", "value": 1.0 }
            }),
        }),
    }]);

    // A landing whose tail is exactly `[fire(raiseAlarm)]`, resumed the same
    // way the scheduler resumes any tail — direct
    // `fire_prepartitioned_reactions_with_sequences` call, `ResumedTail`
    // origin, no `NetEndpoint`/client type anywhere in this call graph.
    let follow_ups = fire_prepartitioned_reactions_with_sequences(
        &[
            postretro_scripting_core::reaction_dispatch::PrepartitionedReactionStep::Descriptor(
                "reveal".to_string(),
                0,
                ReactionDescriptor::Sequence(vec![fx.fire_step("raiseAlarm")]),
            ),
        ],
        &fx.sequence_registry,
        &fx.reaction_registry,
        &fx.system_registry,
        &fx.ctx,
        ResidualOrigin::ResumedTail,
    );
    assert_eq!(
        follow_ups,
        vec!["raiseAlarm".to_string()],
        "the tail's `fire` step names its target for the next dispatch hop"
    );
    // `setState` on a system reaction enqueues onto `ScriptCtx::system_commands`
    // for the app's per-frame drain — draining that queue is outside this
    // scheduler-focused test. What is proven here is the scheduler-owned half
    // of "host-only": the resume path reaches the target reaction through
    // ordinary `postretro`/`scripting-core` calls with no networking type ever
    // in scope, exactly the same call graph a single-player run takes.
}

// ---------------------------------------------------------------------------
// O32 (crossing half): a segmented reaction fired by the host crossing stage
// (`dispatch_state_crossings_with_sequences`) enrolls through the SAME control
// arm as any other named dispatch — no site-specific handling exists because
// none of these sites is enumerated. The UI half of O32
// (`fire_focused_button_activation`'s `on_press`/`on_commit`) has no headless
// harness and is a review gate (see the final report).
// ---------------------------------------------------------------------------
#[test]
fn crossing_fired_reaction_containing_a_wait_enrolls_and_lands_later() {
    let mut fx = Fixture::new();
    let target = fx.spawn_entity();

    let watched_slot = "gate.open".to_string();
    fx.ctx
        .slot_table
        .borrow_mut()
        .insert(
            watched_slot.clone(),
            postretro_entities::SlotRecord::new(postretro_entities::SlotSchema {
                slot_type: postretro_entities::SlotType::Number,
                default: Some(postretro_entities::SlotValue::Number(0.0)),
                range: None,
                persist: false,
                readonly: false,
                ownership: postretro_entities::SlotOwnership::Mod,
                network: postretro_entities::ReplicationScope::None,
                per_owner: false,
                accumulate: None,
            }),
        )
        .expect("fixture slot is vacant");

    let reactions = vec![NamedReaction {
        name: "segmentedReveal".to_string(),
        descriptor: ReactionDescriptor::Sequence(vec![
            fx.note_step(target, "pre"),
            SequenceStep {
                id: SequenceTarget::Wait,
                primitive: "wait".to_string(),
                args: serde_json::json!({ "durationMs": 17.0, "interruptible": false }),
            },
            fx.note_step(target, "post"),
        ]),
    }];
    let crossings = vec![postretro_entities::CrossingDescriptor {
        slot: Some(watched_slot.clone()),
        condition: postretro_entities::CrossingCondition::Above { threshold: 0.5 },
        max: 1.0,
        edge: None,
        fire: vec!["segmentedReveal".to_string()],
    }];
    fx.ctx
        .data_registry
        .borrow_mut()
        .populate_level(reactions.clone(), crossings, &[]);
    fx.data.populate_level(reactions, Vec::new(), &[]);

    let mut detector = postretro_scripting_core::state_crossings::CrossingDetector::new();
    detector.initialize(
        &fx.ctx.data_registry.borrow(),
        &fx.ctx.slot_table.borrow(),
        &fx.ctx,
    );

    // Flip the watched slot: the crossing rises and fires `segmentedReveal`
    // through the exact production dispatch adapter the host crossing stage
    // calls each frame.
    fx.ctx
        .slot_table
        .borrow_mut()
        .get_mut(&watched_slot)
        .unwrap()
        .value = Some(postretro_entities::SlotValue::Number(1.0));
    let fired = crate::scripting::reactions::dispatch_state_crossings_with_sequences(
        &mut detector,
        &fx.ctx.slot_table.borrow(),
        &fx.data,
        &fx.sequence_registry,
        &fx.reaction_registry,
        &fx.system_registry,
        &fx.ctx,
    );
    assert_eq!(fired, vec!["segmentedReveal".to_string()]);
    assert_eq!(
        fx.log(),
        vec!["pre".to_string()],
        "the pre-wait step runs synchronously inside the crossing dispatch"
    );
    assert_eq!(
        fx.scheduler.pending_len(),
        1,
        "the wait enrolled through the same control arm as any other named dispatch"
    );

    // The instance enrolled during the CURRENT frame (crossing detection is
    // part of the frame-end drain in production), so it must not advance
    // while that same frame's `evaluate` still runs — the frame-counter stamp
    // that makes every enrollment phase behave alike (O12/O51). `evaluate`
    // alone (no `begin_frame`) models "still inside this frame".
    fx.scheduler.evaluate(&[]);
    fx.drain();
    assert_eq!(
        fx.log(),
        vec!["pre".to_string()],
        "no advance in the enrollment frame"
    );
    // The NEXT frame's ticks do advance it; a 17ms wait is 2 ticks.
    fx.tick_and_frame();
    fx.tick_and_frame();
    fx.drain();
    assert_eq!(
        fx.log(),
        vec!["pre".to_string(), "post".to_string()],
        "the tail lands at a later frame's drain"
    );
}

// ---------------------------------------------------------------------------
// O44: the crossing stage is frame-sampled. Two landings resume on the SAME
// tick — one writes a watched slot up, the other writes it back down in the
// SAME `drain_landings()` call — and a crossing detector sampled once, after
// the drain settles, observes no rise: the intra-frame round trip never
// crosses.
// ---------------------------------------------------------------------------
#[test]
fn intra_frame_round_trip_write_fires_no_crossing() {
    let mut fx = Fixture::new();
    let target = fx.spawn_entity();
    let watched_slot = "gate.flag".to_string();
    fx.ctx
        .slot_table
        .borrow_mut()
        .insert(
            watched_slot.clone(),
            postretro_entities::SlotRecord::new(postretro_entities::SlotSchema {
                slot_type: postretro_entities::SlotType::Number,
                default: Some(postretro_entities::SlotValue::Number(0.0)),
                range: None,
                persist: false,
                readonly: false,
                ownership: postretro_entities::SlotOwnership::Mod,
                network: postretro_entities::ReplicationScope::None,
                per_owner: false,
                accumulate: None,
            }),
        )
        .expect("fixture slot is vacant");

    // A `writeSlot` sequence primitive that writes the watched slot directly
    // (a stand-in for a resumed `setState` step, which enqueues onto
    // `ScriptCtx::system_commands` for a drain outside this scheduler-focused
    // test — see the delayed-consequential-landing test above for that split).
    {
        let slot_table = fx.ctx.slot_table.clone();
        let watched_slot = watched_slot.clone();
        fx.sequence_registry
            .register("writeSlot", move |_id, args| {
                let value = args
                    .get("value")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0);
                slot_table
                    .borrow_mut()
                    .get_mut(&watched_slot)
                    .expect("watched slot present")
                    .value = Some(postretro_entities::SlotValue::Number(value as f32));
                Ok(())
            });
    }
    fx.install(vec![]);

    let mut detector = postretro_scripting_core::state_crossings::CrossingDetector::new();
    fx.ctx.data_registry.borrow_mut().populate_level(
        Vec::new(),
        vec![postretro_entities::CrossingDescriptor {
            slot: Some(watched_slot.clone()),
            condition: postretro_entities::CrossingCondition::Above { threshold: 0.5 },
            max: 1.0,
            edge: None,
            fire: vec!["gateOpened".to_string()],
        }],
        &[],
    );
    detector.initialize(
        &fx.ctx.data_registry.borrow(),
        &fx.ctx.slot_table.borrow(),
        &fx.ctx,
    );
    assert!(
        detector.detect(&fx.ctx.slot_table.borrow()).is_empty(),
        "baseline (0.0) arms the watcher without firing"
    );

    let write = |v: f64| SequenceStep {
        id: SequenceTarget::Entity(target),
        primitive: "writeSlot".to_string(),
        args: serde_json::json!({ "value": v }),
    };
    // "raise" enrolls first (lower InstanceId) so it lands and runs BEFORE
    // "lower" in the same drain — 0.0 -> 1.0 -> 0.0 within one `drain_landings`.
    fx.enroll("raise", 0, None, vec![write(1.0)], 1, false);
    fx.enroll("lower", 0, None, vec![write(0.0)], 1, false);
    fx.tick_and_frame();
    fx.drain();
    assert_eq!(
        fx.ctx.slot_table.borrow().get(&watched_slot).unwrap().value,
        Some(postretro_entities::SlotValue::Number(0.0)),
        "the settled end-of-frame value is 0.0 (the transient 1.0 never observed)"
    );

    // The crossing stage runs once per frame, after the drain — it compares
    // settled values only, so this intra-frame round trip fires nothing.
    assert!(
        detector.detect(&fx.ctx.slot_table.borrow()).is_empty(),
        "a same-frame write-then-write-back round trip must not fire a crossing"
    );
}

// ---------------------------------------------------------------------------
// O26 / O26b: a landing executes at the drain, AFTER the accumulator pass
// that runs once per tick — an accumulator only observes a landed write on
// the NEXT tick's pass, the same offset a trigger residual's state write
// produces today. And a landed write to an accumulated slot bumps
// `write_generation` and rebases the accumulator's `precise_value`, so the
// prior sub-f32 accumulation is discarded rather than carried forward.
// ---------------------------------------------------------------------------
#[test]
fn landed_write_is_invisible_to_the_same_ticks_accumulator_pass_and_rebases_the_next() {
    use crate::scripting_systems::slot_accumulators::{
        SlotAccumulatorBindings, evaluate_slot_accumulators,
    };

    let fx = Fixture::new();
    let slot = "encounter.progress".to_string();
    fx.ctx
        .slot_table
        .borrow_mut()
        .insert(
            slot.clone(),
            postretro_entities::SlotRecord::new(postretro_entities::SlotSchema {
                slot_type: postretro_entities::SlotType::Number,
                default: Some(postretro_entities::SlotValue::Number(0.0)),
                range: None,
                persist: false,
                readonly: false,
                ownership: postretro_entities::SlotOwnership::Mod,
                network: postretro_entities::ReplicationScope::None,
                per_owner: false,
                // Accumulates `@dt * 10.0` per tick.
                accumulate: Some(postretro_foundation::IrNode::Mul {
                    a: Box::new(postretro_foundation::IrNode::Input {
                        name: "@dt".to_string(),
                        owner: None,
                    }),
                    b: Box::new(postretro_foundation::IrNode::Const {
                        value: postretro_foundation::IrValue::Number(10.0),
                    }),
                }),
            }),
        )
        .expect("fixture slot is vacant");
    let mut bindings = SlotAccumulatorBindings::default();
    bindings.rebuild(&fx.ctx);

    // "Tick 1's" accumulator pass: 0.0 + 0.5*10 = 5.0.
    evaluate_slot_accumulators(&mut bindings, 0.5);
    assert_eq!(
        fx.ctx.slot_table.borrow().get(&slot).unwrap().value,
        Some(postretro_entities::SlotValue::Number(5.0))
    );

    // A landed step's write happens at the drain — chronologically AFTER
    // tick 1's accumulator pass already ran and is therefore invisible to it
    // (O26). `tick_and_frame` + `drain` stand in for that drain phase; the
    // write itself uses the same `write_store_slot` a resumed `setState`
    // step's app-drain applies through (see the delayed-consequential-landing
    // test above), called directly here since this test is about the
    // ACCUMULATOR pass's timing relative to the drain, not the scheduler's
    // own dispatch mechanics (already covered by that other test).
    fx.tick_and_frame();
    fx.drain();
    postretro_scripting_core::store_bridge::write_store_slot(
        &fx.ctx,
        &slot,
        postretro_entities::SlotValue::Number(2.5),
    )
    .expect("landed write applies");
    assert_eq!(
        fx.ctx.slot_table.borrow().get(&slot).unwrap().value,
        Some(postretro_entities::SlotValue::Number(2.5)),
        "the landed write is visible immediately in the slot table itself"
    );

    // "Tick 2's" (the next frame's first tick's) accumulator pass: the
    // write_generation bump makes it rebase from 2.5 (discarding the earlier
    // 5.0 sub-f32 accumulation) rather than continuing from 5.0.
    evaluate_slot_accumulators(&mut bindings, 0.5);
    assert_eq!(
        fx.ctx.slot_table.borrow().get(&slot).unwrap().value,
        Some(postretro_entities::SlotValue::Number(7.5)),
        "rebased from the landed 2.5 (+0.5*10), not continued from the pre-landing 5.0"
    );
}

// ---------------------------------------------------------------------------
// O47 / O48 (crossing site half): `dispatch_state_crossings_with_sequences`
// captures its `fire`/`on_complete` chained names and dispatches them — where
// today (pre-Task-1) all such sites discarded the return value. A crossing
// fires a `Primitive` reaction carrying `on_complete`; the chained reaction
// now runs, proving reactivation at this specific site.
//
// GAP: the OTHER O47 site the AC names, `levelLoad` at install
// (`install_world_cpu`, exercised headlessly via `run_headless_inner`), is
// NOT covered by a test in this file — building that harness needs a real
// `.prl`-loading or `install_world_cpu`-direct fixture (the `startup::lifecycle`
// test module's `test_app()`/`level_world()` pattern), which this task did not
// reach given its scope. See the final report.
// ---------------------------------------------------------------------------
#[test]
fn crossing_fired_on_complete_chain_now_runs() {
    let mut fx = Fixture::new();
    let target = fx.spawn_entity();
    let watched_slot = "gate.chain".to_string();
    fx.ctx
        .slot_table
        .borrow_mut()
        .insert(
            watched_slot.clone(),
            postretro_entities::SlotRecord::new(postretro_entities::SlotSchema {
                slot_type: postretro_entities::SlotType::Number,
                default: Some(postretro_entities::SlotValue::Number(0.0)),
                range: None,
                persist: false,
                readonly: false,
                ownership: postretro_entities::SlotOwnership::Mod,
                network: postretro_entities::ReplicationScope::None,
                per_owner: false,
                accumulate: None,
            }),
        )
        .expect("fixture slot is vacant");

    let reactions = vec![
        NamedReaction {
            name: "primWithComplete".to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "note".to_string(),
                target: None,
                tag: Some("chain-tag".to_string()),
                on_complete: Some("chainedReaction".to_string()),
                args: serde_json::json!({ "label": "primWithComplete" }),
            }),
        },
        NamedReaction {
            name: "chainedReaction".to_string(),
            descriptor: ReactionDescriptor::Sequence(vec![fx.note_step(target, "chainedReaction")]),
        },
    ];
    let crossings = vec![postretro_entities::CrossingDescriptor {
        slot: Some(watched_slot.clone()),
        condition: postretro_entities::CrossingCondition::Above { threshold: 0.5 },
        max: 1.0,
        edge: None,
        fire: vec!["primWithComplete".to_string()],
    }];
    fx.ctx
        .registry
        .borrow_mut()
        .set_tags(target, vec!["chain-tag".to_string()])
        .expect("fixture entity accepts the chain tag");
    fx.ctx
        .data_registry
        .borrow_mut()
        .populate_level(reactions.clone(), crossings, &[]);
    fx.data.populate_level(reactions, Vec::new(), &[]);

    let mut detector = postretro_scripting_core::state_crossings::CrossingDetector::new();
    detector.initialize(
        &fx.ctx.data_registry.borrow(),
        &fx.ctx.slot_table.borrow(),
        &fx.ctx,
    );

    fx.ctx
        .slot_table
        .borrow_mut()
        .get_mut(&watched_slot)
        .unwrap()
        .value = Some(postretro_entities::SlotValue::Number(1.0));
    let fired = crate::scripting::reactions::dispatch_state_crossings_with_sequences(
        &mut detector,
        &fx.ctx.slot_table.borrow(),
        &fx.data,
        &fx.sequence_registry,
        &fx.reaction_registry,
        &fx.system_registry,
        &fx.ctx,
    );
    assert_eq!(fired, vec!["primWithComplete".to_string()]);
    assert_eq!(
        fx.log(),
        vec![
            "primWithComplete".to_string(),
            "chainedReaction".to_string()
        ],
        "the fired reaction ran AND its on_complete chain reactivated — both would be \
         silently dropped if this call site still discarded the return value"
    );
}
