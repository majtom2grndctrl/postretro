// Host-only scheduler for timed/delayed reaction steps (E18).
// See: context/plans/in-progress/E18--timed-reaction-steps/index.md

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use postretro_entities::{DataRegistry, EntityId, ReactionDescriptor, ScriptCtx};
use postretro_scripting_core::data_descriptors::SequenceStep;
use postretro_scripting_core::reaction_dispatch::{
    PrepartitionedReactionStep, ResidualOrigin, dispatch_deferred_named_events_with_sequences,
    fire_prepartitioned_reactions_with_sequences,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
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
pub(crate) struct Landing {
    pub(crate) key: InstanceKey,
    pub(crate) depth: u32,
    pub(crate) tail: Vec<SequenceStep>,
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
    /// enrolls during that deferred dispatch and inherits this depth + 1 (O64,
    /// O65); a nested re-park keeps it unchanged. Wider scope than
    /// `currently_resuming` — depth attribution must reach the deferred phase,
    /// re-park detection must not.
    current_enrollment_depth: Option<u32>,
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

    /// Drop all transient level state without changing the session role. Wired
    /// into `clear_surface_lifetime_level_state` (level teardown and the suspend
    /// path) and the staged-manifest hot-reload commit. A parked instance holds a
    /// snapshot of a body that the next level, a suspend, or an author edit may
    /// invalidate, so a half-completed beat is dropped rather than replayed — the
    /// `warn!` naming the count keeps that distinguishable from a bug (O38-O40).
    pub(crate) fn clear(&self) {
        let mut state = self.state.borrow_mut();
        let dropped = state.instances.len() + state.landings.len();
        if dropped > 0 {
            log::warn!(
                "[Scheduler] dropping {dropped} pending timed-reaction instance(s) on level teardown / suspend / hot reload; a half-completed beat will not resume"
            );
        }
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

    /// Set the origin for the trigger residual currently being drained, cleared
    /// on drop. Held across the residual loop ONLY (released before the deferred
    /// batch — O54). `standing` is whether this origin's paired enter is live at
    /// drain time, feeding the O52 enrollment check. Mirrors `begin_resume`.
    pub(crate) fn begin_origin(
        &self,
        trigger: EntityId,
        player: PlayerId,
        standing: bool,
    ) -> OriginGuard {
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
    pub(crate) fn effective_origin(
        &self,
        address: &str,
        body_ordinal: usize,
    ) -> Option<(EntityId, PlayerId)> {
        let state = self.state.borrow();
        if state.current_origin.is_some() {
            return state.current_origin;
        }
        if let Some(resume_key) = &state.currently_resuming {
            if resume_key.0 == address && resume_key.1 == body_ordinal {
                return resume_key.2;
            }
        }
        None
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
        let key: InstanceKey = (address.to_string(), body_ordinal, origin);
        // O55: a sourceless instance (a `fire`-seeded child, or a batch-seeded
        // enrollment outside the origin guard) has no Exit to cancel on, so an
        // `interruptible` wait is demoted to non-interruptible and warned once at
        // enrollment. V3 makes the install-time version of this decision; this is
        // the runtime path V3's rule already covers.
        let interruptible = if interruptible && origin.is_none() {
            log::warn!(
                "[Scheduler] reaction `{address}` enrolled sourceless (no paired trigger enter); its interruptible wait is treated as non-interruptible"
            );
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
        // O64: a landing instance's slot is freed at expiry — `evaluate` removes it
        // from `instances` before it lands — so a resume-context re-enrollment is
        // NOT holding a slot to reuse. Cap-exemption of these re-enrollments is what
        // lets every nested and `fire`-seeded tail still land in a frame whose new
        // trigger fires already filled the cap.
        let (depth, cap_exempt) = match (&state.currently_resuming, state.current_enrollment_depth)
        {
            // A later wait in the SAME resumed body re-enrolls under the same key
            // WHILE its tail is still being dispatched: the instance continuing,
            // not a new chain link. Keep its depth, cap-exempt, so a 300-wait body
            // is not a chain and re-parks with the cap full (O19b, O57).
            (Some(resume_key), Some(resume_depth)) if *resume_key == key => (resume_depth, true),
            // A `fire`-seeded enrollment during this instance's deferred dispatch
            // (`currently_resuming` already dropped): a new causal link at
            // depth + 1. Cap-exempt too — depth, not the instance cap, bounds
            // `fire` chains (O65).
            (_, Some(parent_depth)) => (parent_depth.saturating_add(1), true),
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
    pub(crate) fn evaluate(&self, exit_fires: &[(EntityId, PlayerId)]) {
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
    pub(crate) fn drop_orphaned_interruptible_instances(
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

    /// Mark the instance whose resumed TAIL is being dispatched, cleared on drop.
    /// Scoped to the tail dispatch ONLY (dropped before the deferred dispatch), so
    /// `enroll` treats a same-key enrollment reached synchronously here as a
    /// nested re-park (keep depth, cap-exempt) and `effective_origin` lets a
    /// nested wait inherit the instance's origin. Mirrors how Task 5 scopes
    /// `current_origin`.
    fn begin_resume(&self, key: InstanceKey) -> ResumeGuard<'_> {
        self.state.borrow_mut().currently_resuming = Some(key);
        ResumeGuard { scheduler: self }
    }

    /// Set the chain depth attributed to enrollments made during one landing
    /// instance's drain, cleared on drop. Scoped to the WHOLE drain — both the
    /// tail resume and the per-instance deferred dispatch — so a `fire`-seeded
    /// child enrolled in the deferred phase inherits this depth + 1 (O64, O65).
    fn begin_enrollment_depth(&self, depth: u32) -> DepthGuard<'_> {
        self.state.borrow_mut().current_enrollment_depth = Some(depth);
        DepthGuard { scheduler: self }
    }

    /// Resume every landing this frame through the shipped residual path — one
    /// instance at a time, each with its OWN `dispatch_deferred_named_events_with_sequences`
    /// call and its own 256-hop budget, so a `fire`-seeded child's depth is
    /// attributable per landing (O27, O65). The tail wraps in a
    /// `Descriptor(Sequence(tail))` handed to `fire_prepartitioned_reactions_with_sequences`
    /// with `ResidualOrigin::ResumedTail`, which exempts it from the residual
    /// consequential-primitive guards (O62). The scheduler owns its tails as
    /// `Vec<SequenceStep>` and never mints a `TriggerResidualHandle` (O33). Must run
    /// AFTER the trigger follow-up dispatch and OUTSIDE the origin guard.
    pub(crate) fn drain_landings(
        &self,
        data_registry: &DataRegistry,
        sequence_registry: &SequencedPrimitiveRegistry,
        reaction_registry: &ReactionPrimitiveRegistry,
        system_registry: &SystemReactionRegistry,
        script_ctx: &ScriptCtx,
    ) {
        for landing in self.take_landings() {
            // Depth attribution spans the WHOLE landing — the tail resume AND the
            // per-instance deferred dispatch — so a `fire`-seeded child (which
            // enrolls during the deferred dispatch) inherits this instance's
            // depth + 1.
            let _depth = self.begin_enrollment_depth(landing.depth);
            let (address, body_ordinal, _origin) = landing.key.clone();
            let follow_ups = {
                // Re-park detection spans ONLY the tail resume. A nested `wait`
                // re-enters synchronously here and is the same instance continuing
                // (same key: keep depth, cap-exempt). This guard MUST drop before
                // the deferred dispatch below: a body that `fire`s its own name
                // re-enters during that dispatch with the identical key, and would
                // be misread as a nested re-park — depth would never increment and
                // a self-fire loop would run unbounded (O28). The narrower scope
                // for re-park than for depth is the whole point of the split; see
                // the `currently_resuming` field doc.
                let _resume = self.begin_resume(landing.key);
                fire_prepartitioned_reactions_with_sequences(
                    &[PrepartitionedReactionStep::Descriptor(
                        address,
                        body_ordinal,
                        ReactionDescriptor::Sequence(landing.tail),
                    )],
                    sequence_registry,
                    reaction_registry,
                    system_registry,
                    script_ctx,
                    ResidualOrigin::ResumedTail,
                )
            };
            dispatch_deferred_named_events_with_sequences(
                follow_ups,
                data_registry,
                sequence_registry,
                reaction_registry,
                system_registry,
                script_ctx,
            );
        }
    }

    /// The current monotonic frame counter (test observability).
    #[cfg(test)]
    pub(crate) fn current_frame(&self) -> u64 {
        self.state.borrow().frame_counter
    }

    /// Number of parked (not-yet-landed) instances (test observability).
    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
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

/// Clears the scheduler's `currently_resuming` cell when one landing instance's
/// tail dispatch finishes — narrower than `DepthGuard`, so it drops before the
/// deferred dispatch. Holds a borrow of the scheduler struct (not a `RefCell`
/// borrow), so `enroll` may freely borrow the interior state while it is alive.
struct ResumeGuard<'a> {
    scheduler: &'a ReactionScheduler,
}

impl Drop for ResumeGuard<'_> {
    fn drop(&mut self) {
        self.scheduler.state.borrow_mut().currently_resuming = None;
    }
}

/// Clears the scheduler's `current_enrollment_depth` cell when one landing
/// instance's whole drain finishes — wider than `ResumeGuard`, spanning the tail
/// resume and the per-instance deferred dispatch. Holds a borrow of the scheduler
/// struct (not a `RefCell` borrow), so `enroll` may freely borrow the interior
/// state while it is alive.
struct DepthGuard<'a> {
    scheduler: &'a ReactionScheduler,
}

impl Drop for DepthGuard<'_> {
    fn drop(&mut self) {
        self.scheduler.state.borrow_mut().current_enrollment_depth = None;
    }
}

/// Clears the scheduler's `current_origin` when one trigger residual's drain
/// finishes. Held across the residual loop iteration in `main.rs` so a `wait`
/// reached synchronously through `fire_prepartitioned_reactions_with_sequences`
/// keys to this origin, and released before the deferred batch runs so a
/// batch-seeded `fire` is sourceless (O54). Owns a scheduler clone (an `Rc`
/// clone), so it does not hold a borrow of the session while the residual runs.
pub(crate) struct OriginGuard {
    scheduler: ReactionScheduler,
}

impl Drop for OriginGuard {
    fn drop(&mut self) {
        let mut state = self.scheduler.state.borrow_mut();
        state.current_origin = None;
        state.current_origin_standing = false;
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
        let scheduler = enabled_scheduler();
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
            let scheduler = enabled_scheduler();
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
            let scheduler = enabled_scheduler();
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
        assert_eq!(
            scheduler.pending_len(),
            2,
            "distinct ordinals, distinct keys"
        );
    }

    // Client / disabled scheduler refuses enrollment: no tail parks, no tail
    // lands (O24 / O43 mechanism at the host-only guard).
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
        scheduler.enroll("plate", 0, None, vec![step(1)], 5, false);
        assert_eq!(
            scheduler.instance_depth(&("plate".to_string(), 0, None)),
            Some(0)
        );
    }

    // O57 / O19b mechanism: a nested re-park (same key while a resume is live)
    // keeps the resuming instance's depth and is exempt from the cap. O65
    // mechanism: a `fire`-seeded enrollment (a different key while a resume is
    // live) inherits depth + 1, also cap-exempt.
    #[test]
    fn resume_context_governs_depth_and_cap_exemption() {
        let scheduler = enabled_scheduler();
        let resuming_key = ("body".to_string(), 0, None);
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
            // depth + 1 = 4, cap-exempt.
            scheduler.enroll("child", 0, None, vec![step(3)], 5, false);
            assert_eq!(
                scheduler.instance_depth(&("child".to_string(), 0, None)),
                Some(4),
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
        let scheduler = enabled_scheduler();
        // A `fire`-seeded child enrolls during the deferred dispatch, when only
        // the depth scope is live (the re-park guard has dropped).
        let _depth = scheduler.begin_enrollment_depth(MAX_REACTION_CHAIN_DEPTH);
        // depth + 1 = MAX + 1 > MAX: dropped.
        scheduler.enroll("child", 0, None, vec![step(1)], 5, false);
        assert_eq!(
            scheduler.instance_depth(&("child".to_string(), 0, None)),
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
        (address.to_string(), ordinal, Some((t, p)))
    }

    // O3: an interruptible instance whose paired Exit arrives before the countdown
    // elapses is cancelled; its consequence never applies.
    #[test]
    fn interruptible_exit_before_landing_cancels() {
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
        // No origin guard is live (the batch runs after each residual's guard drops).
        let alpha = scheduler.effective_origin("alpha", 0);
        assert_eq!(alpha, None, "no origin resolves outside the guard");
        scheduler.enroll("alpha", 0, alpha, vec![step(1)], 5, true);
        let beta = scheduler.effective_origin("beta", 0);
        scheduler.enroll("beta", 0, beta, vec![step(2)], 5, true);
        assert_eq!(
            scheduler.instance_interruptible(&("alpha".to_string(), 0, None)),
            Some(false),
        );
        assert_eq!(
            scheduler.instance_interruptible(&("beta".to_string(), 0, None)),
            Some(false),
        );
    }

    // O55: a reaction reached by the `fire` path enrolls sourceless and is demoted,
    // while the same reaction's Enter-bound instance keeps its cancel source.
    #[test]
    fn fire_path_instance_demoted_enter_bound_keeps_source() {
        let scheduler = enabled_scheduler();
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
            scheduler.instance_interruptible(&("R".to_string(), 0, None)),
            Some(false),
            "the fire-path instance is demoted",
        );
    }

    // O56: two same-named bodies, keyed by distinct ordinals, re-fire without
    // cancelling each other.
    #[test]
    fn two_same_named_bodies_refire_independently() {
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
        {
            let _depth = scheduler.begin_enrollment_depth(0);
            let _resume = scheduler.begin_resume(("S".to_string(), 0, None));
            let resolved = scheduler.effective_origin("S", 0);
            scheduler.enroll("S", 0, resolved, vec![step(1)], 5, false);
        }
        {
            let _depth = scheduler.begin_enrollment_depth(0);
            let _resume = scheduler.begin_resume(("S".to_string(), 1, None));
            let resolved = scheduler.effective_origin("S", 1);
            scheduler.enroll("S", 1, resolved, vec![step(2)], 5, false);
        }
        assert!(scheduler.instance_id(&("S".to_string(), 0, None)).is_some());
        assert!(scheduler.instance_id(&("S".to_string(), 1, None)).is_some());
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
        let scheduler = enabled_scheduler();
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
        let scheduler = enabled_scheduler();
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
}
