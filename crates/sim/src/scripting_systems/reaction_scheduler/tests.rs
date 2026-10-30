// A host's `wait` scheduler: tick counting, the same-key re-fire rule, the
// chain-depth bound and instance cap, interruptible cancel, and landing order
// on the tick timeline. Most rows drive the scheduler by hand; the last drive
// whole frames of several ticks through the real landing drain.
// See: context/lib/scripting.md §12 (Steps after a `wait`)

use super::*;
use postretro_entities::{NamedReaction, SystemReactionCommand};
use postretro_scripting_core::data_descriptors::ReactionDescriptor;
use postretro_scripting_core::reaction_dispatch::{
    NamedEventDispatchContext, dispatch_deferred_named_events_with_sequences,
    fire_named_event_with_sequences,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use serde_json::json;

use crate::scripting_systems::system_reactions::register_system_reaction_primitives;

/// Test observability over the scheduler's private state.
impl ReactionScheduler {
    /// The current monotonic frame counter (test observability).
    pub(crate) fn current_frame(&self) -> u64 {
        self.state.borrow().frame_counter
    }

    /// Chain depth of a parked instance by key (test observability).
    fn instance_depth(&self, key: &InstanceKey) -> Option<u32> {
        self.state
            .borrow()
            .instances
            .get(key)
            .map(|instance| instance.depth)
    }

    /// Monotonic id of a parked instance by key (test observability). A fresh id
    /// after a re-fire proves the prior instance was cancelled and replaced.
    fn instance_id(&self, key: &InstanceKey) -> Option<u64> {
        self.state
            .borrow()
            .instances
            .get(key)
            .map(|instance| instance.id)
    }

    /// Remaining tick countdown of a parked instance by key (test observability).
    fn instance_remaining(&self, key: &InstanceKey) -> Option<u32> {
        self.state
            .borrow()
            .instances
            .get(key)
            .and_then(|instance| match instance.countdown {
                Countdown::Ticks(remaining) => Some(remaining),
                Countdown::Due(_) => None,
            })
    }

    /// Fixed due tick of a parked instance by key (test observability): set
    /// for an enrollment reached during a landing's drain.
    fn instance_due(&self, key: &InstanceKey) -> Option<DueTick> {
        self.state
            .borrow()
            .instances
            .get(key)
            .and_then(|instance| match instance.countdown {
                Countdown::Due(due) => Some(due),
                Countdown::Ticks(_) => None,
            })
    }

    /// Interruptibility of a parked instance by key (test observability). Reflects
    /// the wait currently parked at, after any sourceless demotion.
    fn instance_interruptible(&self, key: &InstanceKey) -> Option<bool> {
        self.state
            .borrow()
            .instances
            .get(key)
            .map(|instance| instance.interruptible)
    }

    /// Stored tail length of a parked instance by key (test observability).
    fn instance_tail_len(&self, key: &InstanceKey) -> Option<usize> {
        self.state
            .borrow()
            .instances
            .get(key)
            .map(|instance| instance.tail.len())
    }

    /// Due ticks of the queued landings, sorted (test observability). On the
    /// host every due tick is local.
    fn queued_due_ticks(&self) -> Vec<u64> {
        let state = self.state.borrow();
        let mut ticks: Vec<u64> = state
            .landings
            .iter()
            .map(|queued| {
                assert_eq!(queued.landing.due.clock, TickClock::Local);
                queued.landing.due.tick
            })
            .collect();
        ticks.sort_unstable();
        ticks
    }
}

fn step(id: u32) -> SequenceStep {
    SequenceStep {
        id: postretro_entities::SequenceTarget::Entity(EntityId::from_raw(id)),
        primitive: "setLightAnimation".to_string(),
        args: json!({}),
    }
}

fn host_scheduler() -> ReactionScheduler {
    let scheduler = ReactionScheduler::default();
    scheduler.set_role(SchedulerRole::Host);
    scheduler
}

// Convert ms→ticks the same way the control handler does (integer micros
// against a 16_667us tick), so the row tests read against real durations.
fn ticks_for(duration_ms: f64) -> u32 {
    let micros = duration_ms * 1000.0;
    (((micros / 16_667.0).ceil()) as u32).max(1)
}

// O1 / O31: a 800ms wait resolves to 48 ticks and lands at the drain of the
// frame containing the 48th host tick, counting the first tick of the first
// frame after install as tick 1.
#[test]
fn eight_hundred_ms_lands_on_the_forty_eighth_tick_frame() {
    assert_eq!(ticks_for(800.0), 48);
    let scheduler = host_scheduler();
    // Enrolled at install: frame counter 0, so the stamp is 0 and the first
    // frame (counter 1) advances it.
    scheduler.enroll("levelLoad", 0, None, vec![step(1)], ticks_for(800.0), false);

    // One tick per frame: 47 frames elapse with no landing.
    for frame in 1..=47 {
        scheduler.begin_frame();
        scheduler.evaluate(&[]);
        assert!(
            scheduler.take_landings().is_empty(),
            "no landing before the 48th tick (frame {frame})"
        );
    }
    scheduler.begin_frame();
    scheduler.evaluate(&[]);
    let landings = scheduler.take_landings();
    assert_eq!(landings.len(), 1, "presB lands at the 48th tick frame");
    assert_eq!(landings[0].tail.len(), 1);
    assert_eq!(scheduler.pending_len(), 0);
}

// O2: a 5ms wait resolves to 1 tick and resumes at the drain of the next
// frame that delivers a tick (never in its own enrollment frame).
#[test]
fn sub_tick_wait_resolves_to_one_tick_and_lands_next_frame() {
    assert_eq!(ticks_for(5.0), 1);
    let scheduler = host_scheduler();
    scheduler.enroll("levelLoad", 0, None, vec![step(1)], ticks_for(5.0), false);
    scheduler.begin_frame();
    scheduler.evaluate(&[]);
    assert_eq!(scheduler.take_landings().len(), 1);
}

// O31: install-time enrollment lands at the first frame that delivers a
// tick, identically for first frames of 1, 3, and 14 ticks.
#[test]
fn install_enrollment_lands_first_tick_frame_for_any_first_frame_tick_count() {
    for ticks_in_first_frame in [1_usize, 3, 14] {
        let scheduler = host_scheduler();
        scheduler.enroll("levelLoad", 0, None, vec![step(1)], ticks_for(17.0), false);
        assert_eq!(ticks_for(17.0), 2);
        // Frame 1 delivers `ticks_in_first_frame` ticks; a 2-tick wait needs
        // the second advancing tick, so with >= 2 ticks it lands in frame 1,
        // and with 1 tick it lands in frame 2 — but it NEVER advances in the
        // enrollment frame (frame 0). Here we assert the invariant that the
        // first advancing evaluate happens in frame 1, whatever its tick count.
        scheduler.begin_frame();
        scheduler.evaluate(&[]);
        // After one tick of frame 1: remaining went 2 -> 1 (advanced, not
        // skipped), so no landing yet.
        assert!(scheduler.take_landings().is_empty());
        for _ in 1..ticks_in_first_frame {
            scheduler.evaluate(&[]);
        }
        if ticks_in_first_frame >= 2 {
            assert_eq!(
                scheduler.take_landings().len(),
                1,
                "a 2-tick wait lands within a first frame of {ticks_in_first_frame} ticks"
            );
        } else {
            assert!(scheduler.take_landings().is_empty());
            scheduler.begin_frame();
            scheduler.evaluate(&[]);
            assert_eq!(scheduler.take_landings().len(), 1);
        }
    }
}

// O12 / O51: an instance enrolled DURING a frame (its counter already
// advanced) does not advance during that frame's ticks — for 1, 3, and 14
// tick counts — and first advances on the next frame's first tick.
#[test]
fn enrollment_never_advances_in_its_own_frame() {
    for ticks_this_frame in [1_usize, 3, 14] {
        let scheduler = host_scheduler();
        // Frame F begins, then a pre-loop enrollment stamps the current frame.
        scheduler.begin_frame();
        scheduler.enroll("uiPress", 0, None, vec![step(1)], ticks_for(5.0), false);
        // None of frame F's ticks advance it (stamp == current frame).
        for _ in 0..ticks_this_frame {
            scheduler.evaluate(&[]);
            assert!(scheduler.take_landings().is_empty());
        }
        assert_eq!(scheduler.pending_len(), 1);
        // Frame F+1's first tick advances and lands the 1-tick wait.
        scheduler.begin_frame();
        scheduler.evaluate(&[]);
        assert_eq!(scheduler.take_landings().len(), 1);
    }
}

// O14: a frame with zero ticks runs no `evaluate`; countdowns are unchanged
// and the landing queue drains but is empty.
#[test]
fn zero_tick_frame_leaves_countdowns_unchanged_and_lands_nothing() {
    let scheduler = host_scheduler();
    scheduler.enroll("levelLoad", 0, None, vec![step(1)], ticks_for(50.0), false);
    let pending_before = scheduler.pending_len();
    // A zero-tick frame: begin_frame, but no evaluate.
    scheduler.begin_frame();
    assert!(scheduler.take_landings().is_empty());
    assert_eq!(scheduler.pending_len(), pending_before);
}

// O41: two same-named bodies each keyed by their own body ordinal park
// independent instances; neither is conflated with the other.
#[test]
fn two_same_named_bodies_park_independent_instances() {
    let scheduler = host_scheduler();
    scheduler.enroll("levelLoad", 0, None, vec![step(1)], ticks_for(50.0), false);
    scheduler.enroll("levelLoad", 1, None, vec![step(2)], ticks_for(50.0), false);
    assert_eq!(
        scheduler.pending_len(),
        2,
        "distinct ordinals, distinct keys"
    );
}

// A scheduler with no stated role refuses enrollment: no tail parks, no tail
// lands (O24 / O43).
#[test]
fn disabled_scheduler_refuses_enrollment() {
    let scheduler = ReactionScheduler::default();
    scheduler.enroll("levelLoad", 0, None, vec![step(1)], ticks_for(50.0), false);
    assert_eq!(scheduler.pending_len(), 0);
    scheduler.begin_frame();
    scheduler.evaluate(&[]);
    assert!(scheduler.take_landings().is_empty());
}

// O61 mechanism (counter half): a scheduler whose counter never advances
// skips its instance forever; advancing it via begin_frame lets the wait land.
#[test]
fn counter_must_advance_or_the_instance_is_skipped_forever() {
    let scheduler = host_scheduler();
    scheduler.begin_frame();
    assert_eq!(
        scheduler.current_frame(),
        1,
        "one begin_frame advanced it once"
    );
    scheduler.enroll("levelLoad", 0, None, vec![step(1)], ticks_for(5.0), false);
    // Counter frozen (begin_frame never called again): evaluate always skips.
    for _ in 0..10 {
        scheduler.evaluate(&[]);
        assert!(scheduler.take_landings().is_empty());
    }
    assert_eq!(
        scheduler.current_frame(),
        1,
        "evaluate never advances the counter"
    );
    assert_eq!(scheduler.pending_len(), 1);
    // Advance the counter: now it lands.
    scheduler.begin_frame();
    assert_eq!(scheduler.current_frame(), 2);
    scheduler.evaluate(&[]);
    assert_eq!(scheduler.take_landings().len(), 1);
}

// O10: enrollment past the per-level cap warns and drops; already-parked
// instances are unaffected. Distinct ordinals so each is its own instance.
#[test]
fn cap_drops_excess_enrollments_and_leaves_parked_instances_untouched() {
    let scheduler = host_scheduler();
    for ordinal in 0..MAX_PENDING_REACTION_INSTANCES {
        scheduler.enroll("levelLoad", ordinal, None, vec![step(1)], 5, false);
    }
    assert_eq!(scheduler.pending_len(), MAX_PENDING_REACTION_INSTANCES);
    scheduler.enroll(
        "levelLoad",
        MAX_PENDING_REACTION_INSTANCES,
        None,
        vec![step(2)],
        5,
        false,
    );
    assert_eq!(
        scheduler.pending_len(),
        MAX_PENDING_REACTION_INSTANCES,
        "an over-cap enrollment is dropped; the parked instances stay",
    );
}

// O8 / O25: two instances landing on one tick drain in ascending InstanceId
// order — the enrollment order, not the BTreeMap key order.
#[test]
fn landings_drain_in_ascending_instance_id_order() {
    let scheduler = host_scheduler();
    // "b" enrolls first (id 0), "a" second (id 1); key order is a < b, but the
    // landing order must follow InstanceId: b then a.
    scheduler.enroll("b", 0, None, vec![step(1)], 1, false);
    scheduler.enroll("a", 0, None, vec![step(2)], 1, false);
    scheduler.begin_frame();
    scheduler.evaluate(&[]);
    let landings = scheduler.take_landings();
    assert_eq!(landings.len(), 2);
    assert_eq!(landings[0].key.0, "b", "id 0 (enrolled first) lands first");
    assert_eq!(landings[1].key.0, "a");
}

// O38 / O39: a lifecycle clear drops every parked and queued instance. The
// count `warn!` is emitted by `clear`; the drop is asserted structurally.
#[test]
fn clear_drops_all_pending_instances() {
    let scheduler = host_scheduler();
    scheduler.enroll("levelLoad", 0, None, vec![step(1)], 5, false);
    scheduler.enroll("levelLoad", 1, None, vec![step(2)], 5, false);
    assert_eq!(scheduler.pending_len(), 2);
    scheduler.clear();
    assert_eq!(scheduler.pending_len(), 0);
    scheduler.begin_frame();
    scheduler.evaluate(&[]);
    assert!(
        scheduler.take_landings().is_empty(),
        "no cleared tail lands"
    );
}

// O28b: a fresh (non-resume) enrollment starts at chain depth zero.
#[test]
fn fresh_enrollment_starts_at_depth_zero() {
    let scheduler = host_scheduler();
    scheduler.enroll("plate", 0, None, vec![step(1)], 5, false);
    assert_eq!(
        scheduler.instance_depth(&("plate".to_string(), 0, TailClass::HostOnly, None)),
        Some(0)
    );
}

// O57 / O19b mechanism: a nested re-park (same key while a resume is live)
// keeps the resuming instance's depth and is exempt from the cap. O65
// mechanism: a `fire`-seeded enrollment during deferred dispatch inherits
// depth + 1, but is a new instance and remains cap-tested.
#[test]
fn resume_context_governs_depth_and_cap_exemption() {
    let scheduler = host_scheduler();
    let resuming_key = ("body".to_string(), 0, TailClass::HostOnly, None);
    // Fill the cap so any cap-tested enrollment would be dropped.
    for ordinal in 0..MAX_PENDING_REACTION_INSTANCES {
        scheduler.enroll("filler", ordinal, None, vec![step(1)], 5, false);
    }
    assert_eq!(scheduler.pending_len(), MAX_PENDING_REACTION_INSTANCES);
    {
        // Depth attribution spans the whole landing drain.
        let _depth = scheduler.begin_enrollment_depth(3);
        {
            // Re-park detection is live only during the tail dispatch. A
            // same-key enrollment here is a nested re-park — depth stays 3,
            // cap-exempt.
            let _resume = scheduler.begin_resume(resuming_key.clone());
            scheduler.enroll("body", 0, None, vec![step(2)], 5, false);
            assert_eq!(scheduler.instance_depth(&resuming_key), Some(3));
        }
        // The tail dispatch is over; the deferred dispatch runs with only the
        // depth scope live. A different-key `fire`-seeded child here inherits
        // depth + 1 = 4, but the full pending cap drops the new instance.
        scheduler.enroll("child", 0, None, vec![step(3)], 5, false);
        assert_eq!(
            scheduler.instance_depth(&("child".to_string(), 0, TailClass::HostOnly, None)),
            None,
            "a fire-seeded child inherits depth for its chain check but remains cap-tested",
        );
    }
    // The guard released the resume context; a fresh enrollment is cap-tested
    // again and dropped (cap still full).
    let before = scheduler.pending_len();
    scheduler.enroll("post", 0, None, vec![step(4)], 5, false);
    assert_eq!(
        scheduler.pending_len(),
        before,
        "a post-resume enrollment is cap-tested again",
    );
}

// O28 mechanism: a `fire`-seeded child past MAX_REACTION_CHAIN_DEPTH warns and
// drops. Depth bounds a causal chain the concurrency cap cannot see.
#[test]
fn fire_seeded_child_past_max_chain_depth_is_dropped() {
    let scheduler = host_scheduler();
    // A `fire`-seeded child enrolls during the deferred dispatch, when only
    // the depth scope is live (the re-park guard has dropped).
    let _depth = scheduler.begin_enrollment_depth(MAX_REACTION_CHAIN_DEPTH);
    // depth + 1 = MAX + 1 > MAX: dropped.
    scheduler.enroll("child", 0, None, vec![step(1)], 5, false);
    assert_eq!(
        scheduler.instance_depth(&("child".to_string(), 0, TailClass::HostOnly, None)),
        None,
        "an over-depth fire-seeded child is dropped",
    );
}

// ---- Task 5: interruptible cancellation, origin, and re-fire ----

fn trigger(id: u32) -> EntityId {
    EntityId::from_raw(id)
}

fn player(id: u64) -> PlayerId {
    PlayerId::Remote(id)
}

fn origin_key(address: &str, ordinal: usize, t: EntityId, p: PlayerId) -> InstanceKey {
    (
        address.to_string(),
        ordinal,
        TailClass::HostOnly,
        Some((t, p)),
    )
}

// O3: an interruptible instance whose paired Exit arrives before the countdown
// elapses is cancelled; its consequence never applies.
#[test]
fn interruptible_exit_before_landing_cancels() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(1)], 5, true);
    scheduler.begin_frame();
    scheduler.evaluate(&[(t, p)]);
    assert_eq!(
        scheduler.pending_len(),
        0,
        "interruptible instance cancelled"
    );
    assert!(
        scheduler.take_landings().is_empty(),
        "no landing after cancel"
    );
}

// O4: an Exit on the exact tick the countdown would reach zero still wins —
// cancels apply before the countdown advance within `evaluate`.
#[test]
fn interruptible_exit_on_landing_tick_cancels() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    // A 1-tick countdown: without the cancel this evaluate would expire it.
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(1)], 1, true);
    scheduler.begin_frame();
    scheduler.evaluate(&[(t, p)]);
    assert_eq!(scheduler.pending_len(), 0);
    assert!(
        scheduler.take_landings().is_empty(),
        "cancel wins over the landing on the exact tick"
    );
}

// O5: a non-interruptible instance ignores its paired Exit during countdown and
// lands on schedule.
#[test]
fn non_interruptible_ignores_exit_and_lands() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(1)], 2, false);
    scheduler.begin_frame();
    scheduler.evaluate(&[(t, p)]); // Exit ignored; 2 -> 1
    assert_eq!(
        scheduler.pending_len(),
        1,
        "non-interruptible not cancelled"
    );
    scheduler.begin_frame();
    scheduler.evaluate(&[(t, p)]); // Exit ignored; 1 -> 0 lands
    assert_eq!(
        scheduler.take_landings().len(),
        1,
        "non-interruptible lands on schedule"
    );
}

// O6: re-firing an interruptible instance while parked cancels it and restarts
// fresh from the top of the body.
#[test]
fn refire_interruptible_cancels_and_restarts() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    let key = origin_key("reveal", 0, t, p);
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(1)], 5, true);
    let first_id = scheduler.instance_id(&key).unwrap();
    scheduler.begin_frame();
    scheduler.evaluate(&[]);
    scheduler.begin_frame();
    scheduler.evaluate(&[]); // 5 -> 3
    assert_eq!(scheduler.instance_remaining(&key), Some(3));
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(2)], 5, true);
    assert_eq!(
        scheduler.pending_len(),
        1,
        "still one instance under the key"
    );
    assert!(
        scheduler.instance_id(&key).unwrap() > first_id,
        "a fresh instance replaced the cancelled one"
    );
    assert_eq!(
        scheduler.instance_remaining(&key),
        Some(5),
        "restart from the top of the body"
    );
}

// O7: re-firing a non-interruptible instance while parked is ignored; the
// running instance completes.
#[test]
fn refire_non_interruptible_is_ignored() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    let key = origin_key("reveal", 0, t, p);
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(1)], 5, false);
    let first_id = scheduler.instance_id(&key).unwrap();
    scheduler.begin_frame();
    scheduler.evaluate(&[]); // 5 -> 4
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(2)], 5, false);
    assert_eq!(
        scheduler.instance_id(&key),
        Some(first_id),
        "running instance kept, re-fire ignored"
    );
    assert_eq!(
        scheduler.instance_remaining(&key),
        Some(4),
        "countdown not reset"
    );
}

// O11: a per-player plate entered by two players yields one instance per key;
// each player's Exit cancels only their own.
#[test]
fn two_players_one_plate_each_exit_cancels_own() {
    let scheduler = host_scheduler();
    let t = trigger(1);
    let (p1, p2) = (player(1), player(2));
    scheduler.enroll("reveal", 0, Some((t, p1)), vec![step(1)], 5, true);
    scheduler.enroll("reveal", 0, Some((t, p2)), vec![step(2)], 5, true);
    assert_eq!(
        scheduler.pending_len(),
        2,
        "one instance per (trigger, player)"
    );
    scheduler.begin_frame();
    scheduler.evaluate(&[(t, p1)]);
    assert_eq!(scheduler.pending_len(), 1, "only P1 cancelled");
    assert!(
        scheduler
            .instance_id(&origin_key("reveal", 0, t, p2))
            .is_some(),
        "P2's instance survives"
    );
}

// O59: one plate, two players, one residual handle — the drain enrolls two
// instances with DISTINCT origins because `TickEvents.trigger_residuals`
// carries `(trigger, player)` beside each handle. Carrying the handle alone
// would collapse both to one instance.
#[test]
fn two_distinct_origins_from_one_handle_yield_two_instances() {
    let scheduler = host_scheduler();
    let t = trigger(7);
    let (p1, p2) = (player(1), player(2));
    // Two enrollments for one binding (one residual handle), distinct origins.
    scheduler.enroll("reveal", 0, Some((t, p1)), vec![step(1)], 5, true);
    scheduler.enroll("reveal", 0, Some((t, p2)), vec![step(2)], 5, true);
    assert_eq!(
        scheduler.pending_len(),
        2,
        "distinct origins never collapse to one instance"
    );
}

// O16: one reaction bound to two triggers yields two instances (the key
// includes the trigger); each trigger's Exit cancels only its own.
#[test]
fn one_reaction_two_triggers_two_instances() {
    let scheduler = host_scheduler();
    let (t1, t2) = (trigger(1), trigger(2));
    let p = player(1);
    scheduler.enroll("R", 0, Some((t1, p)), vec![step(1)], 5, true);
    scheduler.enroll("R", 0, Some((t2, p)), vec![step(2)], 5, true);
    assert_eq!(scheduler.pending_len(), 2, "key includes the trigger");
    scheduler.begin_frame();
    scheduler.evaluate(&[(t1, p)]);
    assert_eq!(scheduler.pending_len(), 1);
    assert!(
        scheduler.instance_id(&origin_key("R", 0, t2, p)).is_some(),
        "T2's instance survives T1's Exit"
    );
}

// O23: a player's death or disconnect fires the paired Exit and cancels an
// interruptible instance, identically to walking off the plate.
#[test]
fn death_or_disconnect_cancels_via_paired_exit() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(1)], 5, true);
    scheduler.begin_frame();
    // The pawn leaving the capsule set fires the same paired Exit a walk-off
    // does; the scheduler sees only `(trigger, player)`.
    scheduler.evaluate(&[(t, p)]);
    assert_eq!(scheduler.pending_len(), 0, "death/disconnect cancels");
}

// O17/O18: the wait currently parked at governs both Exit-cancel and re-fire.
// Parked at a non-interruptible wait (modelled by a non-interruptible
// instance), a paired Exit does not cancel and a re-fire is ignored — the same
// rule as O5/O7, so the two never disagree.
#[test]
fn parked_at_non_interruptible_wait_governs() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    let key = origin_key("body", 0, t, p);
    scheduler.enroll("body", 0, Some((t, p)), vec![step(9)], 5, false);
    let id = scheduler.instance_id(&key).unwrap();
    scheduler.begin_frame();
    scheduler.evaluate(&[(t, p)]);
    assert_eq!(
        scheduler.pending_len(),
        1,
        "non-interruptible wait ignores Exit"
    );
    scheduler.enroll("body", 0, Some((t, p)), vec![step(8)], 5, false);
    assert_eq!(
        scheduler.instance_id(&key),
        Some(id),
        "re-fire ignored while parked at a non-interruptible wait"
    );
}

// O18: parked at an interruptible wait, a re-fire is accepted and restarts the
// body from the top.
#[test]
fn parked_at_interruptible_wait_accepts_refire() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    let key = origin_key("body", 0, t, p);
    scheduler.enroll("body", 0, Some((t, p)), vec![step(9)], 5, true);
    let id = scheduler.instance_id(&key).unwrap();
    scheduler.enroll("body", 0, Some((t, p)), vec![step(8)], 5, true);
    assert!(
        scheduler.instance_id(&key).unwrap() > id,
        "re-fire restarts a fresh instance from the top"
    );
}

// O18b: an accepted re-fire re-runs the body forward; the scheduler holds no
// step inverse, so effects an earlier run applied are not reverted. At this
// layer that means the fresh instance simply carries the whole body tail.
#[test]
fn refire_reruns_forward_without_reverting() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    let key = origin_key("body", 0, t, p);
    scheduler.enroll("body", 0, Some((t, p)), vec![step(1)], 5, true);
    scheduler.enroll(
        "body",
        0,
        Some((t, p)),
        vec![step(1), step(2), step(3)],
        5,
        true,
    );
    assert_eq!(
        scheduler.instance_tail_len(&key),
        Some(3),
        "the fresh instance re-runs the full body forward; nothing is rolled back"
    );
}

// O19: a re-fire at a full cap still re-enrolls — the same-key cancel is applied
// before the cap test, so re-enrollment never counts against the cap.
#[test]
fn refire_at_full_cap_still_reenrolls() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    let key = origin_key("reveal", 0, t, p);
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(1)], 5, true);
    for ordinal in 0..(MAX_PENDING_REACTION_INSTANCES - 1) {
        scheduler.enroll("filler", ordinal, None, vec![step(1)], 5, false);
    }
    assert_eq!(scheduler.pending_len(), MAX_PENDING_REACTION_INSTANCES);
    let first_id = scheduler.instance_id(&key).unwrap();
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(2)], 5, true);
    assert_eq!(
        scheduler.pending_len(),
        MAX_PENDING_REACTION_INSTANCES,
        "cap unchanged; the re-fire cancelled its own slot first"
    );
    assert!(
        scheduler.instance_id(&key).unwrap() > first_id,
        "re-enrolled fresh rather than being dropped at the cap"
    );
}

// O52: an interruptible instance whose origin's paired enter has already left
// by enrollment time does not park — otherwise it would be uncancellable.
#[test]
fn enrollment_refused_when_paired_enter_already_left() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    {
        // Residual drain sets the origin with standing = false (entered and
        // left within one frame). The control arm resolves the same origin.
        let _origin = scheduler.begin_origin(t, p, false);
        let resolved = scheduler.effective_origin("reveal", 0);
        scheduler.enroll("reveal", 0, resolved, vec![step(1)], 5, true);
    }
    assert_eq!(
        scheduler.pending_len(),
        0,
        "no uncancellable instance parks"
    );
}

// O52 companion: a standing paired enter parks; a non-interruptible enrollment
// is unaffected by the standing check even when its origin has left.
#[test]
fn standing_enter_parks_and_non_interruptible_skips_the_check() {
    let scheduler = host_scheduler();
    let t = trigger(1);
    let p1 = player(1);
    {
        let _origin = scheduler.begin_origin(t, p1, true);
        let resolved = scheduler.effective_origin("reveal", 0);
        scheduler.enroll("reveal", 0, resolved, vec![step(1)], 5, true);
    }
    assert_eq!(scheduler.pending_len(), 1, "standing enter parks");
    let p2 = player(2);
    {
        let _origin = scheduler.begin_origin(t, p2, false);
        let resolved = scheduler.effective_origin("reveal", 0);
        scheduler.enroll("reveal", 0, resolved, vec![step(2)], 5, false);
    }
    assert_eq!(
        scheduler.pending_len(),
        2,
        "non-interruptible parks regardless of standing"
    );
}

// O53: an Exit that arrives between an instance's expiry and the frame-end
// drain still cancels — the landing queue is checked alongside parked
// instances. Sharpens O4.
#[test]
fn exit_between_expiry_and_drain_cancels_queued_landing() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(1)], 1, true);
    scheduler.begin_frame();
    scheduler.evaluate(&[]); // tick j: expires into the landing queue, no Exit
    assert_eq!(scheduler.pending_len(), 0, "no longer parked");
    scheduler.evaluate(&[(t, p)]); // tick k > j of the same frame: Exit arrives
    assert!(
        scheduler.take_landings().is_empty(),
        "the queued-but-unrun landing is cancelled before the drain"
    );
}

// Regression (O6/O7): a same-key re-fire that arrives after an instance
// expired into the landing queue but before the frame-end drain must dedup
// against the queued landing. Before the fix the re-fire path checked only
// `instances` (empty by then) and parked a fresh instance beside the queued
// landing, so the tail landed twice.
#[test]
fn refire_after_expiry_before_drain_does_not_double_land_non_interruptible() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(1)], 1, false);
    scheduler.begin_frame();
    scheduler.evaluate(&[]); // expires into the landing queue
    assert_eq!(scheduler.pending_len(), 0, "no longer parked; it is queued");
    // Re-fire the same key before the landing drains: a non-interruptible
    // queued landing ignores the re-fire (O7), exactly as a parked one would.
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(2)], 1, false);
    assert_eq!(
        scheduler.pending_len(),
        0,
        "the re-fire is ignored; no fresh instance parks beside the queued landing",
    );
    assert_eq!(
        scheduler.take_landings().len(),
        1,
        "the tail lands exactly once, not twice",
    );
}

// O6 companion of the row above: an interruptible re-fire in the same window
// cancels the queued landing and restarts fresh — the tail still lands once.
#[test]
fn refire_after_expiry_before_drain_cancels_queued_landing_interruptible() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(1)], 1, true);
    scheduler.begin_frame();
    scheduler.evaluate(&[]); // expires into the landing queue
    assert_eq!(scheduler.pending_len(), 0, "no longer parked; it is queued");
    // Re-fire the same key: the queued interruptible landing is cancelled and a
    // fresh instance restarts from the top of the body (O6).
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(2)], 5, true);
    assert_eq!(
        scheduler.take_landings().len(),
        0,
        "the queued landing was cancelled by the re-fire; it does not also land",
    );
    assert_eq!(
        scheduler.pending_len(),
        1,
        "a fresh instance restarted from the top",
    );
}

// O54: enrollments seeded by a deferred batch run outside the origin guard, so
// they resolve sourceless and their interruptible waits are demoted.
#[test]
fn deferred_batch_enrollments_are_sourceless_and_demoted() {
    let scheduler = host_scheduler();
    // No origin guard is live (the batch runs after each residual's guard drops).
    let alpha = scheduler.effective_origin("alpha", 0);
    assert_eq!(alpha, None, "no origin resolves outside the guard");
    scheduler.enroll("alpha", 0, alpha, vec![step(1)], 5, true);
    let beta = scheduler.effective_origin("beta", 0);
    scheduler.enroll("beta", 0, beta, vec![step(2)], 5, true);
    assert_eq!(
        scheduler.instance_interruptible(&("alpha".to_string(), 0, TailClass::HostOnly, None)),
        Some(false),
    );
    assert_eq!(
        scheduler.instance_interruptible(&("beta".to_string(), 0, TailClass::HostOnly, None)),
        Some(false),
    );
}

// O55: a reaction reached by the `fire` path enrolls sourceless and is demoted,
// while the same reaction's Enter-bound instance keeps its cancel source.
#[test]
fn fire_path_instance_demoted_enter_bound_keeps_source() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    {
        let _origin = scheduler.begin_origin(t, p, true);
        let resolved = scheduler.effective_origin("R", 0);
        scheduler.enroll("R", 0, resolved, vec![step(1)], 5, true);
    }
    // Reached by the `fire` path (no guard live): sourceless, demoted.
    scheduler.enroll("R", 0, None, vec![step(2)], 5, true);
    assert_eq!(
        scheduler.instance_interruptible(&origin_key("R", 0, t, p)),
        Some(true),
        "the Enter-bound instance keeps its cancel source"
    );
    assert_eq!(
        scheduler.instance_interruptible(&("R".to_string(), 0, TailClass::HostOnly, None)),
        Some(false),
        "the fire-path instance is demoted",
    );
}

// O56: two same-named bodies, keyed by distinct ordinals, re-fire without
// cancelling each other.
#[test]
fn two_same_named_bodies_refire_independently() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    scheduler.enroll("levelLoad", 0, Some((t, p)), vec![step(1)], 5, true);
    scheduler.enroll("levelLoad", 1, Some((t, p)), vec![step(2)], 5, true);
    let id0 = scheduler
        .instance_id(&origin_key("levelLoad", 0, t, p))
        .unwrap();
    let id1 = scheduler
        .instance_id(&origin_key("levelLoad", 1, t, p))
        .unwrap();
    // Re-fire body 0: its same-key cancel must not touch body 1.
    scheduler.enroll("levelLoad", 0, Some((t, p)), vec![step(1)], 5, true);
    assert!(
        scheduler
            .instance_id(&origin_key("levelLoad", 0, t, p))
            .unwrap()
            > id0,
        "body 0 restarted"
    );
    assert_eq!(
        scheduler.instance_id(&origin_key("levelLoad", 1, t, p)),
        Some(id1),
        "body 1 untouched"
    );
    assert_eq!(scheduler.pending_len(), 2);
}

// O57: a nested wait in a resumed tail carries its instance's address, ordinal,
// and origin — it stays interruptible, warns nothing, and re-parks even with
// the cap full. Re-deriving would make it sourceless (V3 would strip the flag).
#[test]
fn nested_wait_keeps_provenance_and_stays_interruptible() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    let key = origin_key("R", 2, t, p);
    // Fill the cap so a cap-tested enrollment would be dropped.
    for ordinal in 0..MAX_PENDING_REACTION_INSTANCES {
        scheduler.enroll("filler", ordinal, None, vec![step(1)], 5, false);
    }
    // The instance keyed (R, 2, (t,p)) at depth 4 is resuming its tail: both
    // scopes are live during the tail dispatch, where a nested wait re-enters.
    let _depth = scheduler.begin_enrollment_depth(4);
    let _resume = scheduler.begin_resume(key.clone());
    // The nested wait's control arm resolves the origin from `currently_resuming`.
    let resolved = scheduler.effective_origin("R", 2);
    assert_eq!(
        resolved,
        Some((t, p)),
        "nested wait inherits the instance origin"
    );
    scheduler.enroll("R", 2, resolved, vec![step(9)], 5, true);
    assert_eq!(
        scheduler.instance_interruptible(&key),
        Some(true),
        "stays interruptible, no demotion"
    );
    assert_eq!(
        scheduler.instance_depth(&key),
        Some(4),
        "depth carried unchanged, cap-exempt"
    );
}

// O58: `BodyOrdinal` survives a landing — two same-named bodies each re-enroll
// under their own ordinal, so a re-derived ordinal (which would be 0 for both)
// never conflates them.
#[test]
fn body_ordinal_survives_landing() {
    let scheduler = host_scheduler();
    {
        let _depth = scheduler.begin_enrollment_depth(0);
        let _resume = scheduler.begin_resume(("S".to_string(), 0, TailClass::HostOnly, None));
        let resolved = scheduler.effective_origin("S", 0);
        scheduler.enroll("S", 0, resolved, vec![step(1)], 5, false);
    }
    {
        let _depth = scheduler.begin_enrollment_depth(0);
        let _resume = scheduler.begin_resume(("S".to_string(), 1, TailClass::HostOnly, None));
        let resolved = scheduler.effective_origin("S", 1);
        scheduler.enroll("S", 1, resolved, vec![step(2)], 5, false);
    }
    assert!(
        scheduler
            .instance_id(&("S".to_string(), 0, TailClass::HostOnly, None))
            .is_some()
    );
    assert!(
        scheduler
            .instance_id(&("S".to_string(), 1, TailClass::HostOnly, None))
            .is_some()
    );
    assert_eq!(
        scheduler.pending_len(),
        2,
        "each body re-parks under its own ordinal"
    );
}

// O60: the paired-enter check is membership for a specific `(trigger, player)`,
// never "is anyone standing here". A bystander standing on the plate does not
// rescue an enrollment for a player who already left.
#[test]
fn paired_enter_check_is_per_player_not_per_trigger() {
    let scheduler = host_scheduler();
    let t = trigger(1);
    let p1 = player(1); // entered and left this frame
    let p2 = player(2); // standing since an earlier frame
    // P2's instance parked in an earlier frame while standing.
    scheduler.enroll("reveal", 0, Some((t, p2)), vec![step(2)], 5, true);
    {
        // P1's residual this frame: standing = false for (t, p1) specifically.
        let _origin = scheduler.begin_origin(t, p1, false);
        let resolved = scheduler.effective_origin("reveal", 0);
        scheduler.enroll("reveal", 0, resolved, vec![step(1)], 5, true);
    }
    assert!(
        scheduler
            .instance_id(&origin_key("reveal", 0, t, p1))
            .is_none(),
        "P1's enrollment refused"
    );
    assert!(
        scheduler
            .instance_id(&origin_key("reveal", 0, t, p2))
            .is_some(),
        "P2's standing instance is unaffected"
    );
}

// O63: an interruptible instance whose keyed trigger is removed mid-wait is
// dropped rather than landing uncancelled; a non-interruptible instance on the
// same trigger is untouched, and an instance whose enter still stands is kept.
#[test]
fn orphaned_interruptible_instance_is_dropped_when_trigger_removed() {
    let scheduler = host_scheduler();
    let (t, p) = (trigger(1), player(1));
    let p2 = player(2);
    let (t_live, p_live) = (trigger(9), player(3));
    scheduler.enroll("reveal", 0, Some((t, p)), vec![step(1)], 50, true);
    scheduler.enroll("reveal", 0, Some((t, p2)), vec![step(2)], 50, false);
    scheduler.enroll("live", 0, Some((t_live, p_live)), vec![step(3)], 50, true);
    // Trigger T is gone from `paired_enters`; T_live's enter still stands.
    let standing: BTreeSet<(EntityId, PlayerId)> = BTreeSet::from([(t_live, p_live)]);
    scheduler.drop_orphaned_interruptible_instances(&standing);
    assert!(
        scheduler
            .instance_id(&origin_key("reveal", 0, t, p))
            .is_none(),
        "interruptible instance on the removed trigger dropped"
    );
    assert!(
        scheduler
            .instance_id(&origin_key("reveal", 0, t, p2))
            .is_some(),
        "non-interruptible instance untouched"
    );
    assert!(
        scheduler
            .instance_id(&origin_key("live", 0, t_live, p_live))
            .is_some(),
        "instance whose enter still stands is kept"
    );
}

// ---- Host chains count on the tick timeline ----

/// A host's reaction machinery with `wait`, `fire` and the system steps, run
/// as the frame loop runs it: a frame of fixed ticks, then the landing drain.
struct HostRig {
    ctx: ScriptCtx,
    scheduler: ReactionScheduler,
    sequence: SequencedPrimitiveRegistry,
    reaction: ReactionPrimitiveRegistry,
    system: SystemReactionRegistry,
    /// Sounds played by every landing so far, in order.
    sounds: Vec<String>,
}

impl HostRig {
    fn new(reactions: Vec<NamedReaction>) -> Self {
        let ctx = ScriptCtx::new();
        ctx.data_registry
            .borrow_mut()
            .populate_level(reactions, Vec::new(), &[]);
        let scheduler = host_scheduler();
        let mut sequence = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut sequence, scheduler.clone());
        let mut system = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system);
        Self {
            ctx,
            scheduler,
            sequence,
            reaction: ReactionPrimitiveRegistry::new(),
            system,
            sounds: Vec::new(),
        }
    }

    /// Fire `name` at install, as a host-only source, follow-ups included.
    /// What it plays before any wait is recorded in `sounds`.
    fn fire(&mut self, name: &str) {
        let data = self.ctx.data_registry.borrow();
        let chained = fire_named_event_with_sequences(
            name,
            &data,
            &self.sequence,
            &self.reaction,
            &self.system,
            &self.ctx,
            Some(NamedEventDispatchContext {
                origin: FireOrigin::HostOnly(FireSubject::None),
                source: format!("named:{name}"),
                values: &[],
                emitter: None,
            }),
        );
        dispatch_deferred_named_events_with_sequences(
            chained,
            &data,
            &self.sequence,
            &self.reaction,
            &self.system,
            &self.ctx,
        );
        drop(data);
        self.collect();
    }

    /// One frame of `ticks` fixed ticks, then the frame-end drain. Returns
    /// the due ticks the frame's ticks queued, sorted, and the sounds the
    /// drain played, in order. A landing the drain itself finds overdue is
    /// in the second list only.
    fn frame(&mut self, ticks: usize) -> (Vec<u64>, Vec<String>) {
        self.scheduler.begin_frame();
        for _ in 0..ticks {
            self.scheduler.evaluate(&[]);
        }
        let due = self.scheduler.queued_due_ticks();
        self.scheduler.drain_landings(
            &self.ctx.data_registry.borrow(),
            &self.sequence,
            &self.reaction,
            &self.system,
            &self.ctx,
        );
        let before = self.sounds.len();
        self.collect();
        (due, self.sounds[before..].to_vec())
    }

    fn collect(&mut self) {
        self.sounds.extend(
            self.ctx
                .system_commands
                .take_ordered()
                .into_iter()
                .filter_map(|(_, command)| match command {
                    SystemReactionCommand::PlaySound { sound, .. } => Some(sound),
                    _ => None,
                }),
        );
    }
}

fn host_wait(duration_ms: f64) -> SequenceStep {
    SequenceStep {
        id: postretro_entities::SequenceTarget::Wait,
        primitive: "wait".into(),
        args: json!({ "durationMs": duration_ms, "interruptible": false }),
    }
}
fn host_sound(name: &str) -> SequenceStep {
    SequenceStep {
        id: postretro_entities::SequenceTarget::System,
        primitive: "playSound".into(),
        args: json!({ "sound": name }),
    }
}
fn host_fire(event: &str) -> SequenceStep {
    SequenceStep {
        id: postretro_entities::SequenceTarget::Fire,
        primitive: "fire".into(),
        args: json!({ "event": event }),
    }
}
fn host_body(name: &str, steps: Vec<SequenceStep>) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Sequence(steps),
    }
}

// Host chain drift: a segment a landing parks counts from the tick its parent
// fell due, never from the frame-end drain. Segment 1 (one tick) falls due on
// the first of a frame's three ticks; segment 2's four ticks fall due at tick
// 5, inside the next frame, where counting from the drain (tick 3) would put
// them at tick 7, a frame later. The same holds for a `fire`-seeded child. At
// one tick per frame the timing is unchanged.
#[test]
fn host_chain_segment_counts_from_its_parents_due_tick_not_the_drain() {
    assert_eq!((ticks_for(16.0), ticks_for(60.0)), (1, 4));
    let chained = || {
        vec![host_body(
            "beat",
            vec![
                host_wait(16.0),
                host_sound("a"),
                host_wait(60.0),
                host_sound("b"),
            ],
        )]
    };
    let fired = || {
        vec![
            host_body("beat", vec![host_wait(16.0), host_fire("r")]),
            host_body("r", vec![host_wait(60.0), host_sound("b")]),
        ]
    };
    for (form, reactions) in [("re-park", chained()), ("fire child", fired())] {
        let mut rig = HostRig::new(reactions);
        rig.fire("beat");
        assert_eq!(rig.frame(3).0, [1], "{form}: segment 1 at tick 1");
        assert_eq!(rig.frame(3).0, [5], "{form}: segment 2 at tick 1 + 4");
        assert_eq!(rig.sounds.last().map(String::as_str), Some("b"), "{form}");
        assert_eq!(rig.scheduler.pending_len(), 0, "{form}");
    }
    for (form, reactions) in [("re-park", chained()), ("fire child", fired())] {
        let mut rig = HostRig::new(reactions);
        rig.fire("beat");
        assert_eq!(rig.frame(1).0, [1], "{form}");
        for tick in 2..5 {
            assert!(
                rig.frame(1).0.is_empty(),
                "{form}: nothing due at tick {tick}"
            );
        }
        assert_eq!(rig.frame(1).0, [5], "{form}: one tick per frame");
        assert_eq!(rig.sounds.last().map(String::as_str), Some("b"), "{form}");
    }
}

// A chain lands on the ideal tick timeline even where a segment is shorter
// than a frame. `[wait(16), a, wait(16), b, wait(100), c]` falls due on ticks
// 1, 2 and 8. At three ticks per frame `b` is already past due when `a`'s
// landing parks it, so it lands in the same drain, and `c` counts from `b`'s
// tick 2, not from the drain at tick 3. At one tick per frame each segment
// lands in the frame of its own tick.
#[test]
fn host_chain_lands_each_segment_on_its_ideal_tick() {
    assert_eq!((ticks_for(16.0), ticks_for(100.0)), (1, 6));
    let level = || {
        vec![host_body(
            "levelLoad",
            vec![
                host_wait(16.0),
                host_sound("a"),
                host_wait(16.0),
                host_sound("b"),
                host_wait(100.0),
                host_sound("c"),
            ],
        )]
    };
    let key = ("levelLoad".to_string(), 0, TailClass::HostOnly, None);

    let mut rig = HostRig::new(level());
    rig.fire("levelLoad");
    assert_eq!(
        rig.frame(3),
        (vec![1], vec!["a".to_string(), "b".to_string()]),
        "ticks 1-3: `a` at tick 1, `b` at tick 2 in the same drain"
    );
    assert_eq!(
        rig.scheduler.instance_due(&key),
        Some(DueTick {
            clock: TickClock::Local,
            tick: 8
        }),
        "`c` counts from `b`'s tick 2"
    );
    assert_eq!(rig.frame(3), (vec![], vec![]), "ticks 4-6");
    assert_eq!(
        rig.frame(3),
        (vec![8], vec!["c".to_string()]),
        "ticks 7-9: `c` at tick 8"
    );
    assert_eq!(rig.scheduler.pending_len(), 0);

    let mut rig = HostRig::new(level());
    rig.fire("levelLoad");
    assert_eq!(rig.frame(1), (vec![1], vec!["a".to_string()]));
    assert_eq!(rig.frame(1), (vec![2], vec!["b".to_string()]));
    for tick in 3..8 {
        assert_eq!(rig.frame(1), (vec![], vec![]), "nothing at tick {tick}");
    }
    assert_eq!(rig.frame(1), (vec![8], vec!["c".to_string()]));
    assert_eq!(rig.scheduler.pending_len(), 0);
}

// A self-firing one-tick beat `r = [wait(16), link, fire(r)]` lands one link
// per host tick at any ticks per frame, as a client's host-timeline mirror
// does (`client_tests::client_mirror_of_a_self_fire_lands_one_link_per_host_tick`).
#[test]
fn host_self_fire_lands_one_link_per_tick_whatever_the_ticks_per_frame() {
    for ticks_per_frame in [1, 3] {
        let mut rig = HostRig::new(vec![host_body(
            "r",
            vec![host_wait(16.0), host_sound("link"), host_fire("r")],
        )]);
        rig.fire("r");
        for _ in 0..(30 / ticks_per_frame) {
            rig.frame(ticks_per_frame);
        }
        assert_eq!(
            rig.sounds.len(),
            30,
            "{ticks_per_frame} tick(s) per frame: one link per tick over 30 ticks"
        );
        assert_eq!(rig.scheduler.pending_len(), 1, "the next link is parked");
    }
}

// Within one frame of several ticks, landings resume in due order, and those
// due on one tick in enrollment order (O8, O25): `late` enrolls first but
// falls due on tick 3, after `early` and `early2` on tick 1.
#[test]
fn host_frame_of_several_ticks_lands_in_due_order_then_enrollment_order() {
    assert_eq!(ticks_for(50.0), 3);
    let mut rig = HostRig::new(vec![
        host_body("late", vec![host_wait(50.0), host_sound("late")]),
        host_body("early", vec![host_wait(16.0), host_sound("early")]),
        host_body("early2", vec![host_wait(16.0), host_sound("early2")]),
    ]);
    for name in ["late", "early", "early2"] {
        rig.fire(name);
    }
    assert_eq!(
        rig.frame(3),
        (
            vec![1, 1, 3],
            vec![
                "early".to_string(),
                "early2".to_string(),
                "late".to_string()
            ]
        )
    );
}

// The same-key rule holds against a landing still queued behind an earlier
// one in the same drain (O6/O7). `r`'s instance falls due on tick 2; `x`,
// due on tick 1 of the same frame, fires `r` before it lands. The re-fire is
// ignored, so `r`'s tail lands once.
#[test]
fn host_refire_dedups_against_a_landing_due_later_in_the_same_frame() {
    assert_eq!(ticks_for(32.0), 2);
    let mut rig = HostRig::new(vec![
        host_body("r", vec![host_wait(32.0), host_sound("r")]),
        host_body("x", vec![host_wait(16.0), host_fire("r")]),
    ]);
    rig.fire("r");
    rig.fire("x");
    assert_eq!(rig.frame(3), (vec![1, 2], vec!["r".to_string()]));
    for _ in 0..3 {
        rig.frame(3);
    }
    assert_eq!(rig.sounds, ["r"], "`r`'s tail lands once");
    assert_eq!(rig.scheduler.pending_len(), 0);
}

// A `fire` from a re-parked landing counts from that landing's due tick.
// `main = [a, wait(16), b, wait(50), fire(r), c]`, `r = [wait(32), d]`: `b` at
// tick 1, `c` at tick 4, `d` at tick 6. At three ticks per frame `d` is past
// due when the tick-4 landing fires `r`, so it lands in that same drain.
#[test]
fn host_fire_from_a_re_parked_landing_lands_on_its_ideal_tick() {
    let reactions = || {
        vec![
            host_body(
                "main",
                vec![
                    host_sound("a"),
                    host_wait(16.0),
                    host_sound("b"),
                    host_wait(50.0),
                    host_fire("r"),
                    host_sound("c"),
                ],
            ),
            host_body("r", vec![host_wait(32.0), host_sound("d")]),
        ]
    };
    let mut rig = HostRig::new(reactions());
    rig.fire("main");
    assert_eq!(rig.sounds, ["a"], "the pre-wait step runs at the fire");
    assert_eq!(rig.frame(3), (vec![1], vec!["b".to_string()]));
    assert_eq!(
        rig.frame(3),
        (vec![4], vec!["c".to_string(), "d".to_string()]),
        "ticks 4-6: `c` at tick 4, then `r`'s `d` at tick 6 in the same drain"
    );
    assert_eq!(rig.scheduler.pending_len(), 0);

    let mut rig = HostRig::new(reactions());
    rig.fire("main");
    let landed: Vec<(Vec<u64>, Vec<String>)> = (0..7).map(|_| rig.frame(1)).collect();
    assert_eq!(
        landed,
        [
            (vec![1], vec!["b".to_string()]),
            (vec![], vec![]),
            (vec![], vec![]),
            (vec![4], vec!["c".to_string()]),
            (vec![], vec![]),
            (vec![6], vec!["d".to_string()]),
            (vec![], vec![]),
        ]
    );
}
