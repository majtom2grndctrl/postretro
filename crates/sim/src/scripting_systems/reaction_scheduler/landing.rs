// Frame-end landing drain for the timed-reaction scheduler (E18).
// See: context/lib/scripting.md §12 (Steps after a `wait`)

use postretro_entities::{DataRegistry, ReactionDescriptor, ScriptCtx};
use postretro_scripting_core::reaction_dispatch::{
    PrepartitionedReactionStep, ResidualOrigin, dispatch_deferred_named_events_with_sequences,
    fire_prepartitioned_reactions_with_sequences,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

use super::{InstanceKey, ReactionScheduler};

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
    pub fn drain_landings(
        &self,
        data_registry: &DataRegistry,
        sequence_registry: &SequencedPrimitiveRegistry,
        reaction_registry: &ReactionPrimitiveRegistry,
        system_registry: &SystemReactionRegistry,
        script_ctx: &ScriptCtx,
    ) {
        for landing in self.take_landings() {
            let _depth = self.begin_enrollment_depth(landing.depth);
            let (address, body_ordinal, _origin) = landing.key.clone();
            let previous = script_ctx.system_commands.replace_fire_context(
                postretro_entities::SystemCommandFireContext {
                    source: format!("landing:{address}"),
                    origin: postretro_entities::FireOrigin::HostOnly(
                        postretro_entities::FireSubject::None,
                    ),
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
