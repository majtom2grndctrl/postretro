// Frame-end landing drain for the timed-reaction scheduler (E18).
// See: context/lib/scripting.md §12 (Steps after a `wait`)

use postretro_entities::{
    ClientLanding, DataRegistry, FireOrigin, FireSubject, ReactionDescriptor, ScriptCtx,
};
use postretro_scripting_core::reaction_dispatch::{
    PrepartitionedReactionStep, ResidualOrigin, dispatch_deferred_named_events_with_sequences,
    fire_prepartitioned_reactions_with_sequences,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

use super::{InstanceKey, Landing, ReactionScheduler, SchedulerRole, TailClass};

impl ReactionScheduler {
    /// Mark the instance whose resumed TAIL is being dispatched, cleared on drop.
    /// Scoped to the tail dispatch ONLY (dropped before the deferred dispatch), so
    /// `enroll` treats a same-key enrollment reached synchronously here as a
    /// nested re-park (keep depth, cap-exempt) and `effective_origin` lets a
    /// nested wait inherit the instance's origin. Mirrors how Task 5 scopes
    /// `current_origin`.
    pub(super) fn begin_resume(&self, key: InstanceKey) -> ResumeGuard<'_> {
        self.state.borrow_mut().currently_resuming = Some(key);
        ResumeGuard { scheduler: self }
    }

    /// Set the chain depth attributed to enrollments made during one landing
    /// instance's drain, cleared on drop. Scoped to the WHOLE drain — both the
    /// tail resume and the per-instance deferred dispatch — so a `fire`-seeded
    /// child enrolled in the deferred phase inherits this depth + 1 (O65).
    pub(super) fn begin_enrollment_depth(&self, depth: u32) -> DepthGuard<'_> {
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
    ///
    /// A landing dispatches under the origin its tail parked with, so it routes
    /// presentation as the same step does before the wait. On the host, a
    /// host-only or host-mirrored landing captures light, fog and emitter
    /// writes as host-only overrides, across its tail and its follow-ups; a
    /// client lands only machine-local steps, converging state for a segment a
    /// late joiner found already due.
    pub fn drain_landings(
        &self,
        data_registry: &DataRegistry,
        sequence_registry: &SequencedPrimitiveRegistry,
        reaction_registry: &ReactionPrimitiveRegistry,
        system_registry: &SystemReactionRegistry,
        script_ctx: &ScriptCtx,
    ) {
        let client = self.role() == SchedulerRole::Client;
        loop {
            for landing in self.take_landings() {
                self.drain_one(
                    landing,
                    client,
                    data_registry,
                    sequence_registry,
                    reaction_registry,
                    system_registry,
                    script_ctx,
                );
            }
            // A late joiner's due chain resolves at its first reading. Each pass
            // consumes a wait or a chain-capped `fire`, so the loop ends.
            if !(client && self.expire_late()) {
                break;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn drain_one(
        &self,
        landing: Landing,
        client: bool,
        data_registry: &DataRegistry,
        sequence_registry: &SequencedPrimitiveRegistry,
        reaction_registry: &ReactionPrimitiveRegistry,
        system_registry: &SystemReactionRegistry,
        script_ctx: &ScriptCtx,
    ) {
        let _depth = self.begin_enrollment_depth(landing.depth);
        let (address, body_ordinal, class, _subject) = landing.key.clone();
        let (origin, host_capture, client_landing) = if client {
            let mode = if self.is_late(landing.due_host_tick) {
                ClientLanding::Converge
            } else {
                ClientLanding::Land
            };
            (landing.origin.clone(), false, Some(mode))
        } else {
            // A host-only landing still drops its subject: subject routing at
            // landing re-resolves the seat, which the host-only drains own.
            let origin = match &landing.origin {
                FireOrigin::HostOnly(_) => FireOrigin::HostOnly(FireSubject::None),
                every_machine => every_machine.clone(),
            };
            (origin, class != TailClass::OwnState, None)
        };
        let _anchor = self.begin_host_anchor(landing.due_host_tick);
        let _scope = script_ctx.landing.enter(host_capture, client_landing);
        let previous = script_ctx.system_commands.replace_fire_context(
            postretro_entities::SystemCommandFireContext {
                source: format!("landing:{address}"),
                origin,
                ..postretro_entities::SystemCommandFireContext::default()
            },
        );
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
        script_ctx.system_commands.replace_fire_context(previous);
        // The follow-ups dispatch with their own fire contexts but inside the
        // landing's scope, so a `fire` target lands as the tail does.
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

/// Clears the scheduler's `currently_resuming` cell when one landing instance's
/// tail dispatch finishes — narrower than `DepthGuard`, so it drops before the
/// deferred dispatch. Holds a borrow of the scheduler struct (not a `RefCell`
/// borrow), so `enroll` may freely borrow the interior state while it is alive.
pub(super) struct ResumeGuard<'a> {
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
pub(super) struct DepthGuard<'a> {
    scheduler: &'a ReactionScheduler,
}

impl Drop for DepthGuard<'_> {
    fn drop(&mut self) {
        self.scheduler.state.borrow_mut().current_enrollment_depth = None;
    }
}
