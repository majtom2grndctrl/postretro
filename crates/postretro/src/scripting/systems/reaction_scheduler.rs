// Host-only scheduler for timed/delayed reaction steps (E18).
//
// A `wait` control step inside a `sequence` reaction body enrolls the remainder
// of that body here and stops the synchronous drain. The scheduler counts
// authoritative ticks and, on expiry, moves the stored tail into a landing queue
// that the frame-end drain resumes through the shipped residual path.
//
// Shaped exactly like `MoverAutoCloseTimers`: `#[derive(Debug, Clone, Default)]`
// over an `Rc<RefCell<_>>`, main-thread only, owned on the session beside
// `slot_accumulator_bindings` and cloned into the control handler at
// registration. It intentionally does not participate in snapshots, digests, or
// the connected-client simulation — enrollment is refused host-side by the
// `enabled` latch, so no tail ever parks or lands on a client.
//
// See: context/plans/in-progress/E18--timed-reaction-steps/index.md

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use postretro_entities::EntityId;
use postretro_scripting_core::data_descriptors::SequenceStep;
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

use crate::trigger_system::PlayerId;

/// Register the `wait`/`fire` control primitives on the sequence registry. Each
/// gets an **inert** `handlers` entry — `sequence_primitives_are_valid` consults
/// only that map, so the names must be present there to survive
/// `setupLevel`/`ModManifest` validation. `wait` additionally gets a control
/// handler capturing a `ReactionScheduler` clone; the dispatcher's control arm
/// reads that parallel table and never reaches the inert entry. `fire` is handled
/// inline by the control arm (it collects the target `event` name), so it needs
/// no control handler — only the inert admission entry.
pub(crate) fn register_reaction_control_primitives(
    sequence_registry: &mut SequencedPrimitiveRegistry,
    scheduler: ReactionScheduler,
) {
    sequence_registry.register("wait", |_id, _args| Ok(()));
    sequence_registry.register("fire", |_id, _args| Ok(()));

    sequence_registry.register_control("wait", move |address, body_ordinal, tail, args| {
        let duration_ms = args
            .get("durationMs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let interruptible = args
            .get("interruptible")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let ticks = ms_to_ticks(duration_ms);
        // Task 5 supplies the real origin from the scheduler's `current_origin`;
        // Task 1 enrollments (levelLoad / named / crossing fires) are sourceless.
        scheduler.enroll(
            address,
            body_ordinal,
            None,
            tail.to_vec(),
            ticks,
            interruptible,
        );
    });
}

/// Convert authored milliseconds to a whole-tick countdown at enrollment.
/// `ticks = max(1, ceil(durationMs * 1000 / 16_667))` in integer micros against
/// `TICK_DURATION`. V1 (Task 4) is the sole rejection point for malformed
/// durations; this clamp is defensive so a stray value can never yield a 0-tick
/// or `u32::MAX` countdown before that pass lands.
fn ms_to_ticks(duration_ms: f64) -> u32 {
    let micros = crate::frame_timing::TICK_DURATION.as_micros() as f64;
    if !duration_ms.is_finite() || duration_ms <= 0.0 {
        return 1;
    }
    let ticks = (duration_ms * 1000.0 / micros).ceil();
    if ticks >= u32::MAX as f64 {
        u32::MAX
    } else {
        (ticks as u32).max(1)
    }
}

/// Instance key: `(reaction address, body ordinal, origin)`. The address is the
/// reaction *name* — stable across a `recompose_active_sets` where vector
/// positions are not. The body ordinal keeps two same-named bodies from
/// cancelling each other. Origin is `None` for sourceless (named / `levelLoad` /
/// `fire`-seeded) enrollments; Task 5 populates the trigger-fired case.
pub(crate) type InstanceKey = (String, usize, Option<(EntityId, PlayerId)>);

#[derive(Debug, Clone)]
struct PendingInstance {
    /// Monotonic id assigned at enrollment; the sole landing-order sort key.
    id: u64,
    /// The remainder of the reaction body after the wait step.
    tail: Vec<SequenceStep>,
    /// Whole authoritative ticks still to elapse before landing (>= 1).
    remaining_ticks: u32,
    /// Whether the paired trigger Exit cancels this instance (Task 5 consumes it).
    #[allow(dead_code)]
    interruptible: bool,
    /// The monotonic frame counter at enrollment. `evaluate` skips any instance
    /// whose stamp equals the current frame, so an instance never advances in the
    /// frame that enrolled it — identically for every dispatch phase.
    enrolled_frame: u64,
}

#[derive(Debug, Default)]
struct SchedulerState {
    /// Host/single-player only. Connected clients never enroll.
    enabled: bool,
    /// Monotonic frame counter — advanced by `begin_frame`, never reset. Distinct
    /// from `tick_index`, which resets every frame.
    frame_counter: u64,
    /// Monotonic instance-id source.
    next_instance_id: u64,
    instances: BTreeMap<InstanceKey, PendingInstance>,
    /// Tails whose countdown reached zero, awaiting the frame-end landing drain.
    /// Carries the instance id so the drain resumes in ascending-id order.
    landings: Vec<(u64, Vec<SequenceStep>)>,
}

/// Cloneable session-owned handle. `Rc<RefCell<_>>` is main-thread only, matching
/// `MoverAutoCloseTimers`.
#[derive(Debug, Clone, Default)]
pub(crate) struct ReactionScheduler {
    state: Rc<RefCell<SchedulerState>>,
}

impl ReactionScheduler {
    /// Enable only for the host/single-player session. Connected clients keep no
    /// active scheduler even though the control handler is registered on their
    /// long-lived sequence registry. Clearing on disable mirrors the precedent.
    pub(crate) fn set_enabled(&self, enabled: bool) {
        let mut state = self.state.borrow_mut();
        state.enabled = enabled;
        if !enabled {
            state.instances.clear();
            state.landings.clear();
        }
    }

    /// Drop all transient level state without changing the session role. (Task 3
    /// wires this into the level-lifecycle clears.)
    #[allow(dead_code)]
    pub(crate) fn clear(&self) {
        let mut state = self.state.borrow_mut();
        state.instances.clear();
        state.landings.clear();
    }

    /// Advance the monotonic frame counter once per frame. Three call sites: the
    /// `RedrawRequested` arm, `SimHarness::frame`, and `run_headless_inner`'s
    /// loop. A counter advanced only by window events never advances in the
    /// headless drivers, where every instance would then be skipped forever.
    pub(crate) fn begin_frame(&self) {
        self.state.borrow_mut().frame_counter += 1;
    }

    /// Enroll the tail of a body reached by a `wait` step. Refused host-only at
    /// this entry point (not at a call site), because named fires run on clients
    /// too. `ticks` is the already-converted whole-tick countdown (>= 1); the
    /// ms→ticks conversion happens in the control handler, which lives in the
    /// binary crate and can read `TICK_DURATION`.
    pub(crate) fn enroll(
        &self,
        address: &str,
        body_ordinal: usize,
        origin: Option<(EntityId, PlayerId)>,
        tail: Vec<SequenceStep>,
        ticks: u32,
        interruptible: bool,
    ) {
        let mut state = self.state.borrow_mut();
        if !state.enabled {
            // O24/O43: a client (or any disabled scheduler) parks nothing. Host
            // consequences arrive by replication.
            log::warn!(
                "[Scheduler] wait enrollment refused for reaction `{address}` (scheduler disabled / non-host); the tail will not run here"
            );
            return;
        }
        let id = state.next_instance_id;
        state.next_instance_id += 1;
        let enrolled_frame = state.frame_counter;
        let key: InstanceKey = (address.to_string(), body_ordinal, origin);
        state.instances.insert(
            key,
            PendingInstance {
                id,
                tail,
                remaining_ticks: ticks.max(1),
                interruptible,
                enrolled_frame,
            },
        );
    }

    /// Advance every parked instance by one tick, skipping any enrolled this very
    /// frame. Expired instances move to the landing queue in ascending-id order.
    /// Countdowns are whole ticks, so this takes no `dt`.
    pub(crate) fn evaluate(&self) {
        let mut state = self.state.borrow_mut();
        if !state.enabled {
            return;
        }
        let current = state.frame_counter;
        let mut expired: Vec<InstanceKey> = Vec::new();
        for (key, instance) in state.instances.iter_mut() {
            if instance.enrolled_frame == current {
                continue;
            }
            instance.remaining_ticks = instance.remaining_ticks.saturating_sub(1);
            if instance.remaining_ticks == 0 {
                expired.push(key.clone());
            }
        }
        let mut landed: Vec<(u64, Vec<SequenceStep>)> = Vec::new();
        for key in expired {
            if let Some(instance) = state.instances.remove(&key) {
                landed.push((instance.id, instance.tail));
            }
        }
        landed.sort_by_key(|(id, _)| *id);
        state.landings.extend(landed);
    }

    /// Drain the landing queue, returning each expired instance's tail in
    /// ascending-`InstanceId` order. The frame-end drain wraps each tail in a
    /// `Descriptor(Sequence(tail))` and resumes it through the shipped residual
    /// path. (Task 1 drains it from `SimHarness::frame`; Task 3 wires the
    /// `main.rs` landing drain, which is why the non-test binary has no caller yet.)
    #[allow(dead_code)]
    pub(crate) fn take_landings(&self) -> Vec<Vec<SequenceStep>> {
        let mut state = self.state.borrow_mut();
        let mut landings = std::mem::take(&mut state.landings);
        landings.sort_by_key(|(id, _)| *id);
        landings.into_iter().map(|(_, tail)| tail).collect()
    }

    /// The current monotonic frame counter (test observability).
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn current_frame(&self) -> u64 {
        self.state.borrow().frame_counter
    }

    /// Number of parked (not-yet-landed) instances (test observability).
    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.state.borrow().instances.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(id: u32) -> SequenceStep {
        SequenceStep {
            id: postretro_entities::SequenceTarget::Entity(EntityId::from_raw(id)),
            primitive: "setLightAnimation".to_string(),
            args: json!({}),
        }
    }

    fn enabled_scheduler() -> ReactionScheduler {
        let scheduler = ReactionScheduler::default();
        scheduler.set_enabled(true);
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
        let scheduler = enabled_scheduler();
        // Enrolled at install: frame counter 0, so the stamp is 0 and the first
        // frame (counter 1) advances it.
        scheduler.enroll("levelLoad", 0, None, vec![step(1)], ticks_for(800.0), false);

        // One tick per frame: 47 frames elapse with no landing.
        for frame in 1..=47 {
            scheduler.begin_frame();
            scheduler.evaluate();
            assert!(
                scheduler.take_landings().is_empty(),
                "no landing before the 48th tick (frame {frame})"
            );
        }
        scheduler.begin_frame();
        scheduler.evaluate();
        let landings = scheduler.take_landings();
        assert_eq!(landings.len(), 1, "presB lands at the 48th tick frame");
        assert_eq!(landings[0].len(), 1);
        assert_eq!(scheduler.pending_len(), 0);
    }

    // O2: a 5ms wait resolves to 1 tick and resumes at the drain of the next
    // frame that delivers a tick (never in its own enrollment frame).
    #[test]
    fn sub_tick_wait_resolves_to_one_tick_and_lands_next_frame() {
        assert_eq!(ticks_for(5.0), 1);
        let scheduler = enabled_scheduler();
        scheduler.enroll("levelLoad", 0, None, vec![step(1)], ticks_for(5.0), false);
        scheduler.begin_frame();
        scheduler.evaluate();
        assert_eq!(scheduler.take_landings().len(), 1);
    }

    // O31: install-time enrollment lands at the first frame that delivers a
    // tick, identically for first frames of 1, 3, and 14 ticks.
    #[test]
    fn install_enrollment_lands_first_tick_frame_for_any_first_frame_tick_count() {
        for ticks_in_first_frame in [1_usize, 3, 14] {
            let scheduler = enabled_scheduler();
            scheduler.enroll("levelLoad", 0, None, vec![step(1)], ticks_for(17.0), false);
            assert_eq!(ticks_for(17.0), 2);
            // Frame 1 delivers `ticks_in_first_frame` ticks; a 2-tick wait needs
            // the second advancing tick, so with >= 2 ticks it lands in frame 1,
            // and with 1 tick it lands in frame 2 — but it NEVER advances in the
            // enrollment frame (frame 0). Here we assert the invariant that the
            // first advancing evaluate happens in frame 1, whatever its tick count.
            scheduler.begin_frame();
            scheduler.evaluate();
            // After one tick of frame 1: remaining went 2 -> 1 (advanced, not
            // skipped), so no landing yet.
            assert!(scheduler.take_landings().is_empty());
            for _ in 1..ticks_in_first_frame {
                scheduler.evaluate();
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
                scheduler.evaluate();
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
            let scheduler = enabled_scheduler();
            // Frame F begins, then a pre-loop enrollment stamps the current frame.
            scheduler.begin_frame();
            scheduler.enroll("uiPress", 0, None, vec![step(1)], ticks_for(5.0), false);
            // None of frame F's ticks advance it (stamp == current frame).
            for _ in 0..ticks_this_frame {
                scheduler.evaluate();
                assert!(scheduler.take_landings().is_empty());
            }
            assert_eq!(scheduler.pending_len(), 1);
            // Frame F+1's first tick advances and lands the 1-tick wait.
            scheduler.begin_frame();
            scheduler.evaluate();
            assert_eq!(scheduler.take_landings().len(), 1);
        }
    }

    // O14: a frame with zero ticks runs no `evaluate`; countdowns are unchanged
    // and the landing queue drains but is empty.
    #[test]
    fn zero_tick_frame_leaves_countdowns_unchanged_and_lands_nothing() {
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
        scheduler.enroll("levelLoad", 0, None, vec![step(1)], ticks_for(50.0), false);
        scheduler.enroll("levelLoad", 1, None, vec![step(2)], ticks_for(50.0), false);
        assert_eq!(scheduler.pending_len(), 2, "distinct ordinals, distinct keys");
    }

    // Client / disabled scheduler refuses enrollment: no tail parks, no tail
    // lands (O24 / O43 mechanism at the host-only guard).
    #[test]
    fn disabled_scheduler_refuses_enrollment() {
        let scheduler = ReactionScheduler::default();
        scheduler.enroll("levelLoad", 0, None, vec![step(1)], ticks_for(50.0), false);
        assert_eq!(scheduler.pending_len(), 0);
        scheduler.begin_frame();
        scheduler.evaluate();
        assert!(scheduler.take_landings().is_empty());
    }

    // O61 mechanism (counter half): a scheduler whose counter never advances
    // skips its instance forever; advancing it via begin_frame lets the wait land.
    #[test]
    fn counter_must_advance_or_the_instance_is_skipped_forever() {
        let scheduler = enabled_scheduler();
        scheduler.begin_frame();
        scheduler.enroll("levelLoad", 0, None, vec![step(1)], ticks_for(5.0), false);
        // Counter frozen (begin_frame never called again): evaluate always skips.
        for _ in 0..10 {
            scheduler.evaluate();
            assert!(scheduler.take_landings().is_empty());
        }
        assert_eq!(scheduler.pending_len(), 1);
        // Advance the counter: now it lands.
        scheduler.begin_frame();
        scheduler.evaluate();
        assert_eq!(scheduler.take_landings().len(), 1);
    }
}
