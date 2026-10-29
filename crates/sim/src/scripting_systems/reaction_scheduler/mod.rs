// Host-only scheduler for timed/delayed reaction steps (E18).
// See: context/plans/done/E18--timed-reaction-steps/index.md

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use postretro_entities::EntityId;
use postretro_scripting_core::data_descriptors::SequenceStep;
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

use crate::trigger_system::PlayerId;

/// Cap on concurrent parked instances per level — a cycle breaker, not a content
/// budget. Four players across thirty timed plates is 120 legitimate concurrent
/// instances, so a smaller cap would bite authored co-op content, while a runaway
/// grows without bound either way. Enrollment past the cap warns and drops; the
/// pre-wait steps have already run and the tail is abandoned (O10).
pub(crate) const MAX_PENDING_REACTION_INSTANCES: usize = 256;

/// Bound on the enrolled-by chain depth. A `fire` step in one instance's landing
/// enrolls a child at depth + 1; past this the enrollment warns once and drops
/// (O28). Depth bounds causal chains — self-loops and mutual recursion — where the
/// instance cap cannot, because a one-at-a-time `fire` loop never raises the count.
pub(crate) const MAX_REACTION_CHAIN_DEPTH: u32 = 256;

/// Register the `wait`/`fire` control primitives on the sequence registry. Each
/// gets an **inert** `handlers` entry — `sequence_primitives_are_valid` consults
/// only that map, so the names must be present there to survive
/// `setupLevel`/`ModManifest` validation. `wait` additionally gets a control
/// handler capturing a `ReactionScheduler` clone; the dispatcher's control arm
/// reads that parallel table and never reaches the inert entry. `fire` is handled
/// inline by the control arm (it collects the target `event` name), so it needs
/// no control handler — only the inert admission entry.
pub fn register_reaction_control_primitives(
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
        // The real origin comes from the scheduler's scoped state, never from the
        // control arm (which has no `(trigger, player)` in scope): the trigger
        // residual drain sets `current_origin` per iteration; a nested wait in a
        // resumed tail inherits its instance's origin from `currently_resuming`; every
        // other fire (levelLoad / named / crossing / `fire`-seeded batch) resolves
        // sourceless. Re-deriving here would be wrong — see O54/O55/O57.
        let origin = scheduler.effective_origin(address, body_ordinal);
        scheduler.enroll(
            address,
            body_ordinal,
            origin,
            tail.to_vec(),
            ticks,
            interruptible,
        );
    });
}

/// Convert authored milliseconds to a whole-tick countdown at enrollment.
/// `ticks = max(1, ceil(durationMs * 1000 / 16_667))` in integer micros against
/// `TICK_DURATION`. V1 (Task 4) is the sole rejection point for malformed
/// durations; this clamp is defensive so a stray value never yields a 0-tick
/// countdown and clamps huge finite values to `u32::MAX` rather than overflowing
/// the cast before that pass lands.
pub(crate) fn ms_to_ticks(duration_ms: f64) -> u32 {
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
pub type InstanceKey = (String, usize, Option<(EntityId, PlayerId)>);

#[derive(Debug, Clone)]
struct PendingInstance {
    /// Monotonic id assigned at enrollment; the sole landing-order sort key.
    id: u64,
    /// The remainder of the reaction body after the wait step.
    tail: Vec<SequenceStep>,
    /// Whole authoritative ticks still to elapse before landing (>= 1).
    remaining_ticks: u32,
    /// Whether the paired trigger Exit cancels this instance. Reflects the wait
    /// currently parked at, so in a multi-wait body the parked-at wait governs
    /// both Exit-cancel and re-fire (O17/O18).
    interruptible: bool,
    /// The monotonic frame counter at enrollment. `evaluate` skips any instance
    /// whose stamp equals the current frame, so an instance never advances in the
    /// frame that enrolled it — identically for every dispatch phase.
    enrolled_frame: u64,
    /// Chain depth along the enrolled-by relation. A fresh trigger/named/levelLoad
    /// fire enrolls at 0; a `fire`-seeded child inherits its parent's depth + 1.
    /// Carried on the instance (and on the landing) so depth is attributable
    /// per-instance across a multi-landing batch (O65), never batch-scoped.
    depth: u32,
}

/// One expired instance handed to the frame-end landing drain. Carries the
/// instance key and its chain depth so a nested wait in the resumed tail
/// re-enrolls under the same key (O57/O58) and a `fire`-seeded child is
/// attributed depth + 1 (O65).
#[derive(Debug, Clone)]
pub struct Landing {
    pub key: InstanceKey,
    pub depth: u32,
    pub tail: Vec<SequenceStep>,
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
    /// Carries the instance id (for ascending-id resume order), key, depth, and
    /// the interruptible flag — a queued-but-unrun landing is still cancellable by
    /// a paired Exit that arrives between expiry and the drain (O53).
    landings: Vec<(u64, InstanceKey, u32, bool, Vec<SequenceStep>)>,
    /// Origin of the trigger residual currently being drained, set by the scoped
    /// `OriginGuard` in `main.rs` for the residual loop ONLY (released before the
    /// deferred batch — O54). A `wait` reached while it is live keys its instance
    /// to this `(trigger, player)`; every batch-seeded fire is sourceless.
    current_origin: Option<(EntityId, PlayerId)>,
    /// Whether `current_origin`'s paired enter is still standing at drain time. An
    /// interruptible instance may park only while it is — a player who entered and
    /// left within one frame produced a cancel before anything parked (O52/O60).
    current_origin_standing: bool,
    /// Key of the landing instance whose resumed TAIL is currently being
    /// dispatched — set by the RAII `ResumeGuard` and cleared before that
    /// instance's per-instance deferred dispatch begins. A `wait` reached while it
    /// is live is a nested re-park of the SAME instance (same key: keep depth,
    /// cap-exempt — O19b/O57), and `effective_origin` reads it so a nested wait
    /// inherits the instance's origin.
    ///
    /// Scoped NARROWER than `current_enrollment_depth` on purpose. A nested wait
    /// re-enters synchronously through the control arm *while the tail is being
    /// dispatched*, so this cell is live for it. A self-`fire` child re-enters
    /// through the *deferred* dispatch, which runs AFTER the tail dispatch
    /// completes and this cell has dropped — so it is a fresh fire-seeded child,
    /// not a re-park. Holding this cell across the deferred dispatch too (its
    /// original, wider scope) misread a self-`fire` body — whose child computes
    /// the identical key as the resuming instance — as a re-park, so depth never
    /// incremented and a self-fire loop ran unbounded (O28).
    currently_resuming: Option<InstanceKey>,
    /// Chain depth of the landing instance currently being drained — set by the
    /// RAII `DepthGuard` for the WHOLE of that instance's drain, spanning both the
    /// tail resume and the per-instance deferred dispatch. A `fire`-seeded child
    /// enrolls during that deferred dispatch and inherits this depth + 1 (O65);
    /// a nested re-park keeps it unchanged. Wider scope than
    /// `currently_resuming` — depth attribution must reach the deferred phase,
    /// re-park detection must not.
    current_enrollment_depth: Option<u32>,
    /// Per-level diagnostic latch for connected-client/disabled enrollment
    /// refusal. A reaction can fire repeatedly in a frame, but the lost tail is
    /// actionable only once per reaction until level lifecycle state clears.
    warned_disabled_reactions: BTreeSet<(String, usize)>,
    /// Per-level diagnostic latch for an interruptible wait reached without a
    /// trigger origin. The wait is demoted on every enrollment, but the warning
    /// is emitted once per reaction until level lifecycle state clears.
    warned_sourceless_interruptible_reactions: BTreeSet<(String, usize)>,
}

/// Cloneable session-owned handle to the host-only timed-reaction scheduler. A
/// `wait` control step inside a `sequence` reaction body enrolls the remainder of
/// that body here and stops the synchronous drain; the scheduler counts
/// authoritative ticks and, on expiry, moves the stored tail into a landing queue
/// that the frame-end drain resumes through the shipped residual path.
///
/// Shaped exactly like `MoverAutoCloseTimers`: `Rc<RefCell<_>>`, main-thread only,
/// owned on the session beside `slot_accumulator_bindings` and cloned into the
/// control handler at registration. It does not participate in snapshots, digests,
/// or the connected-client simulation — enrollment is refused host-side by the
/// `enabled` latch, so no tail ever parks or lands on a client.
#[derive(Debug, Clone, Default)]
pub struct ReactionScheduler {
    state: Rc<RefCell<SchedulerState>>,
}

impl ReactionScheduler {
    /// Enable only for the host/single-player session. Connected clients keep no
    /// active scheduler even though the control handler is registered on their
    /// long-lived sequence registry. Clearing on disable mirrors the precedent.
    pub fn set_enabled(&self, enabled: bool) {
        let mut state = self.state.borrow_mut();
        let role_changed = state.enabled != enabled;
        state.enabled = enabled;
        if !enabled {
            state.instances.clear();
            state.landings.clear();
        }
        if role_changed {
            state.warned_disabled_reactions.clear();
            state.warned_sourceless_interruptible_reactions.clear();
        }
    }

    /// Drop all transient level state without changing the session role. Wired
    /// into `clear_surface_lifetime_level_state` (level teardown and the suspend
    /// path) and the staged-manifest hot-reload commit. A parked instance holds a
    /// snapshot of a body that the next level, a suspend, or an author edit may
    /// invalidate, so a half-completed beat is dropped rather than replayed — the
    /// `warn!` naming the count keeps that distinguishable from a bug (O38-O40).
    pub fn clear(&self) {
        let mut state = self.state.borrow_mut();
        let dropped = state.instances.len() + state.landings.len();
        if dropped > 0 {
            log::warn!(
                "[Scheduler] dropping {dropped} pending timed-reaction instance(s) on level teardown / suspend / hot reload; a half-completed beat will not resume"
            );
        }
        state.instances.clear();
        state.landings.clear();
        state.warned_disabled_reactions.clear();
        state.warned_sourceless_interruptible_reactions.clear();
    }

    /// Advance the monotonic frame counter once per frame. Three call sites: the
    /// `RedrawRequested` arm, `SimHarness::frame`, and `run_headless_inner`'s
    /// loop. A counter advanced only by window events never advances in the
    /// headless drivers, where every instance would then be skipped forever.
    pub fn begin_frame(&self) {
        self.state.borrow_mut().frame_counter += 1;
    }

    /// Set the origin for the trigger or player-event residual currently being
    /// drained, cleared on drop. Held across the residual loop ONLY (released
    /// before the deferred batch — O54). `standing` is whether this origin's
    /// paired enter is live at drain time, feeding the O52 enrollment check; a
    /// player event has no paired enter and passes `false`, keyed by the event
    /// player's pawn. Mirrors `begin_resume`.
    pub fn begin_origin(&self, trigger: EntityId, player: PlayerId, standing: bool) -> OriginGuard {
        {
            let mut state = self.state.borrow_mut();
            state.current_origin = Some((trigger, player));
            state.current_origin_standing = standing;
        }
        OriginGuard {
            scheduler: self.clone(),
        }
    }

    /// Resolve the origin a `wait` enrollment should carry. During the residual
    /// drain `current_origin` is set and wins. While a resumed tail is being
    /// dispatched `current_origin` is clear: a nested wait in the SAME body
    /// inherits its instance's origin from `currently_resuming` (so its key
    /// matches and it stays interruptible — O57). A `fire`-seeded child re-enters
    /// through the later deferred dispatch, after `currently_resuming` has
    /// dropped, so it resolves sourceless. Everywhere else (levelLoad / named /
    /// crossing / deferred batch) there is nothing set and the result is `None`.
    pub fn effective_origin(
        &self,
        address: &str,
        body_ordinal: usize,
    ) -> Option<(EntityId, PlayerId)> {
        let state = self.state.borrow();
        if state.current_origin.is_some() {
            return state.current_origin;
        }
        if let Some(resume_key) = &state.currently_resuming
            && resume_key.0 == address
            && resume_key.1 == body_ordinal
        {
            return resume_key.2;
        }
        None
    }

    /// Enroll the tail of a body reached by a `wait` step. Refused host-only at
    /// this entry point (not at a call site), because named fires run on clients
    /// too. `ticks` is the already-converted whole-tick countdown (>= 1); the
    /// ms→ticks conversion happens in the control handler, which lives in the
    /// binary crate and can read `TICK_DURATION`.
    pub fn enroll(
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
            if state
                .warned_disabled_reactions
                .insert((address.to_string(), body_ordinal))
            {
                log::warn!(
                    "[Scheduler] wait enrollment refused for reaction `{address}` (scheduler disabled / non-host); the tail will not run here"
                );
            }
            return;
        }
        let key: InstanceKey = (address.to_string(), body_ordinal, origin);
        // O55: a sourceless instance (a `fire`-seeded child, or a batch-seeded
        // enrollment outside the origin guard) has no Exit to cancel on, so an
        // `interruptible` wait is demoted to non-interruptible and warned once at
        // enrollment. V3 makes the install-time version of this decision; this is
        // the runtime path V3's rule already covers.
        let interruptible = if interruptible && origin.is_none() {
            if state
                .warned_sourceless_interruptible_reactions
                .insert((address.to_string(), body_ordinal))
            {
                log::warn!(
                    "[Scheduler] reaction `{address}` enrolled sourceless (no paired trigger enter); its interruptible wait is treated as non-interruptible"
                );
            }
            false
        } else {
            interruptible
        };
        // O52/O60: an interruptible instance may park only while its origin's
        // paired enter is still standing. During the residual drain `origin`
        // equals `current_origin`; a player who entered and left within the frame
        // is no longer standing, so the instance must not park — otherwise the
        // beat fires with nobody on the plate and no cancel ever arrives.
        if interruptible
            && origin.is_some()
            && origin == state.current_origin
            && !state.current_origin_standing
        {
            log::warn!(
                "[Scheduler] interruptible wait enrollment for `{address}` refused: its origin's paired enter has already left this frame"
            );
            return;
        }
        // O6/O7/O17/O18/O19: same-key re-fire. The wait currently parked at
        // governs — an interruptible parked instance is cancelled and re-enrolls
        // fresh from the top of the body; a non-interruptible one ignores the
        // re-fire. Applied BEFORE the cap test so a re-enrollment never counts
        // against the cap and never strands the pre-wait work (O19). A nested
        // re-park during a resume finds no parked instance under the key (the
        // instance already landed), so this branch is skipped there.
        match state.instances.get(&key).map(|inst| inst.interruptible) {
            Some(true) => {
                state.instances.remove(&key);
            }
            Some(false) => return,
            None => {
                // The instance may have already expired out of `instances` into
                // the landing queue, awaiting the frame-end drain. A same-key
                // re-fire in that window must dedup against the queued landing
                // exactly as it would a still-parked instance, or the landing's
                // tail runs AND the fresh instance's tail runs — the tail lands
                // twice (violates O6/O7). Mirrors how `evaluate` checks `landings`
                // alongside `instances` for the Exit-cancel path (O53).
                match state
                    .landings
                    .iter()
                    .find(|(_, landing_key, _, _, _)| *landing_key == key)
                    .map(|(_, _, _, interruptible, _)| *interruptible)
                {
                    Some(true) => {
                        state
                            .landings
                            .retain(|(_, landing_key, ..)| *landing_key != key);
                    }
                    Some(false) => return,
                    None => {}
                }
            }
        }
        // Depth and cap accounting depend on which phase of a landing's drain we
        // are in. `currently_resuming` is live ONLY while a tail is being
        // dispatched (re-park detection); `current_enrollment_depth` is live for
        // the whole landing including its deferred dispatch (depth attribution).
        // The phase — not the key — is what distinguishes a nested re-park from a
        // self-`fire` child, because a self-`fire` child computes the identical
        // key as the resuming instance yet re-enters one phase later (O28).
        // A landing instance's slot is freed at expiry — `evaluate` removes it
        // from `instances` before it lands — so a genuine nested re-park is not
        // holding a slot to reuse. Only that synchronous continuation is cap
        // exempt. A `fire`-seeded child is a new instance and remains cap-tested,
        // even though it inherits the landing parent's chain depth.
        let (depth, cap_exempt) = match (&state.currently_resuming, state.current_enrollment_depth)
        {
            // A later wait in the SAME resumed body re-enrolls under the same key
            // WHILE its tail is still being dispatched: the instance continuing,
            // not a new chain link. Keep its depth, cap-exempt, so a 300-wait body
            // is not a chain and re-parks with the cap full (O19b, O57).
            (Some(resume_key), Some(resume_depth)) if *resume_key == key => (resume_depth, true),
            // A `fire`-seeded enrollment during this instance's deferred dispatch
            // (`currently_resuming` already dropped): a new causal link at
            // depth + 1 and a new concurrent instance, so the cap still applies.
            (_, Some(parent_depth)) => (parent_depth.saturating_add(1), false),
            // A fresh trigger / levelLoad / named / crossing fire starts at depth
            // zero and is cap-tested (O28b).
            (_, None) => (0, false),
        };
        if depth > MAX_REACTION_CHAIN_DEPTH {
            log::warn!(
                "[Scheduler] reaction `{address}` reached the {MAX_REACTION_CHAIN_DEPTH}-deep enrolled-by chain cap; dropping this enrollment (its pre-wait steps already ran)"
            );
            return;
        }
        if !cap_exempt && state.instances.len() >= MAX_PENDING_REACTION_INSTANCES {
            log::warn!(
                "[Scheduler] wait enrollment for `{address}` exceeds the per-level cap of {MAX_PENDING_REACTION_INSTANCES} concurrent instances; dropping it (its pre-wait steps already ran and the tail is abandoned)"
            );
            return;
        }
        let id = state.next_instance_id;
        state.next_instance_id += 1;
        let enrolled_frame = state.frame_counter;
        state.instances.insert(
            key,
            PendingInstance {
                id,
                tail,
                remaining_ticks: ticks.max(1),
                interruptible,
                enrolled_frame,
                depth,
            },
        );
    }

    /// Advance every parked instance by one tick, skipping any enrolled this very
    /// frame. Expired instances move to the landing queue, which `take_landings`
    /// drains in ascending-id order (O8/O25). Countdowns are whole ticks, so this
    /// takes no `dt`.
    ///
    /// `exit_fires` is this tick's paired-trigger Exit set (`(trigger, player)`).
    /// Cancels apply **before** the countdown advance so an Exit on the exact
    /// landing tick wins (O4); they reach the landing queue too, so an Exit
    /// between an instance's expiry and the frame-end drain still cancels (O53).
    pub fn evaluate(&self, exit_fires: &[(EntityId, PlayerId)]) {
        let mut state = self.state.borrow_mut();
        if !state.enabled {
            return;
        }
        // O4/O53: cancel matching interruptible instances (and queued landings)
        // BEFORE advancing countdowns. A non-interruptible instance ignores its
        // paired Exit (O5), so the flag gates the cancel.
        if !exit_fires.is_empty() {
            state.instances.retain(|key, instance| {
                !(instance.interruptible
                    && key.2.is_some_and(|origin| exit_fires.contains(&origin)))
            });
            state.landings.retain(|(_, key, _, interruptible, _)| {
                !(*interruptible && key.2.is_some_and(|origin| exit_fires.contains(&origin)))
            });
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
        let mut landed: Vec<(u64, InstanceKey, u32, bool, Vec<SequenceStep>)> = Vec::new();
        for key in expired {
            if let Some(instance) = state.instances.remove(&key) {
                landed.push((
                    instance.id,
                    key,
                    instance.depth,
                    instance.interruptible,
                    instance.tail,
                ));
            }
        }
        // No per-tick sort here: `take_landings` re-sorts the whole `landings` vec
        // by id at drain time (O8/O25), so sorting this batch first is dead work.
        state.landings.extend(landed);
    }

    /// Drop any parked interruptible instance whose origin's paired enter is no
    /// longer standing (O63). By construction a parked interruptible instance's
    /// player is standing — the O52 check refuses to park otherwise, and a player
    /// walking off fires a paired Exit that `evaluate` already cancelled on. So a
    /// surviving interruptible instance whose `(trigger, player)` has dropped out
    /// of `paired_enters` means its trigger itself was removed (`paired_enters`
    /// retains only live triggers), leaving no edge to ever cancel on. Drop it
    /// with a `warn!` naming the reaction rather than letting it land uncancelled.
    /// Non-interruptible instances are untouched — they ignore the Exit obligation
    /// (O5) and legitimately outlive a walk-off.
    pub fn drop_orphaned_interruptible_instances(
        &self,
        standing_enters: &BTreeSet<(EntityId, PlayerId)>,
    ) {
        let mut state = self.state.borrow_mut();
        if !state.enabled {
            return;
        }
        state.instances.retain(|key, instance| {
            let orphaned = instance.interruptible
                && key.2.is_some_and(|origin| !standing_enters.contains(&origin));
            if orphaned {
                log::warn!(
                    "[Scheduler] dropping interruptible instance for reaction `{}`: its keyed trigger left the level before the wait elapsed, so no Exit can cancel it",
                    key.0
                );
            }
            !orphaned
        });
        // An instance that expired the same tick its keyed trigger was removed sits
        // in the landing queue, not `instances`; sweep it under the same predicate
        // so it does not land uncancelled (O63).
        state.landings.retain(|(_, key, _, interruptible, _)| {
            !(*interruptible
                && key
                    .2
                    .is_some_and(|origin| !standing_enters.contains(&origin)))
        });
    }

    /// Drain the landing queue, returning each expired instance in
    /// ascending-`InstanceId` order (O8, O25). Prefer [`Self::drain_landings`],
    /// which additionally scopes the per-instance resume context; this primitive
    /// exists for the scheduler's own unit tests.
    pub(crate) fn take_landings(&self) -> Vec<Landing> {
        let mut state = self.state.borrow_mut();
        let mut landings = std::mem::take(&mut state.landings);
        landings.sort_by_key(|(id, ..)| *id);
        landings
            .into_iter()
            .map(|(_, key, depth, _interruptible, tail)| Landing { key, depth, tail })
            .collect()
    }

    /// The current monotonic frame counter (test observability).
    #[cfg(test)]
    pub(crate) fn current_frame(&self) -> u64 {
        self.state.borrow().frame_counter
    }

    /// Number of parked (not-yet-landed) instances (test observability).
    #[cfg(any(test, feature = "test-support"))]
    pub fn pending_len(&self) -> usize {
        self.state.borrow().instances.len()
    }

    /// Chain depth of a parked instance by key (test observability).
    #[cfg(test)]
    fn instance_depth(&self, key: &InstanceKey) -> Option<u32> {
        self.state
            .borrow()
            .instances
            .get(key)
            .map(|instance| instance.depth)
    }

    /// Monotonic id of a parked instance by key (test observability). A fresh id
    /// after a re-fire proves the prior instance was cancelled and replaced.
    #[cfg(test)]
    fn instance_id(&self, key: &InstanceKey) -> Option<u64> {
        self.state
            .borrow()
            .instances
            .get(key)
            .map(|instance| instance.id)
    }

    /// Remaining tick countdown of a parked instance by key (test observability).
    #[cfg(test)]
    fn instance_remaining(&self, key: &InstanceKey) -> Option<u32> {
        self.state
            .borrow()
            .instances
            .get(key)
            .map(|instance| instance.remaining_ticks)
    }

    /// Interruptibility of a parked instance by key (test observability). Reflects
    /// the wait currently parked at, after any sourceless demotion.
    #[cfg(test)]
    fn instance_interruptible(&self, key: &InstanceKey) -> Option<bool> {
        self.state
            .borrow()
            .instances
            .get(key)
            .map(|instance| instance.interruptible)
    }

    /// Stored tail length of a parked instance by key (test observability).
    #[cfg(test)]
    fn instance_tail_len(&self, key: &InstanceKey) -> Option<usize> {
        self.state
            .borrow()
            .instances
            .get(key)
            .map(|instance| instance.tail.len())
    }
}

/// Clears the scheduler's `current_origin` when one trigger residual's drain
/// finishes. Held across the residual loop iteration in `main.rs` so a `wait`
/// reached synchronously through `fire_prepartitioned_reactions_with_sequences`
/// keys to this origin, and released before the deferred batch runs so a
/// batch-seeded `fire` is sourceless (O54). Owns a scheduler clone (an `Rc`
/// clone), so it does not hold a borrow of the session while the residual runs.
pub struct OriginGuard {
    scheduler: ReactionScheduler,
}

impl Drop for OriginGuard {
    fn drop(&mut self) {
        let mut state = self.scheduler.state.borrow_mut();
        state.current_origin = None;
        state.current_origin_standing = false;
    }
}

mod landing;

#[cfg(test)]
mod tests;
