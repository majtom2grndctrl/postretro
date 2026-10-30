// Frame-end landing drain for the timed-reaction scheduler (E18).
// See: context/lib/scripting.md §12 (Steps after a `wait`)

use postretro_entities::{
    ClientLanding, DataRegistry, EntityRegistry, FireOrigin, FireSubject, ReactionDescriptor,
    ScriptCtx,
};
use postretro_foundation::Seat;
use postretro_scripting_core::reaction_dispatch::{
    PrepartitionedReactionStep, ResidualOrigin, dispatch_deferred_named_events_with_sequences,
    fire_prepartitioned_reactions_with_sequences,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

use super::{DueTick, InstanceKey, Landing, ReactionScheduler, SchedulerRole, TailClass};
use crate::trigger_system::PlayerId;

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

    /// Set the due tick every enrollment reached during one landing's drain
    /// counts from — its tail resume and its follow-ups alike — restoring the
    /// previous anchor when the returned guard drops. Scoped like
    /// `begin_enrollment_depth`.
    pub(super) fn begin_anchor(&self, due: DueTick) -> AnchorGuard<'_> {
        let previous = self.state.borrow_mut().current_anchor.replace(due);
        AnchorGuard {
            scheduler: self,
            previous,
        }
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
    ///
    /// Seats resolve from the registry alone, so a remote subject reads as
    /// departed; a session with remote players uses
    /// [`Self::drain_landings_resolving_seats`].
    pub fn drain_landings(
        &self,
        data_registry: &DataRegistry,
        sequence_registry: &SequencedPrimitiveRegistry,
        reaction_registry: &ReactionPrimitiveRegistry,
        system_registry: &SystemReactionRegistry,
        script_ctx: &ScriptCtx,
    ) {
        self.drain_landings_resolving_seats(
            data_registry,
            sequence_registry,
            reaction_registry,
            system_registry,
            script_ctx,
            &|player, registry| match player {
                PlayerId::Local(pawn) => registry.seat_for_pawn(pawn),
                PlayerId::Remote(_) => None,
            },
        );
    }

    /// [`Self::drain_landings`], resolving a host-only landing's subject to
    /// the seat it holds now: the host's own player to `Seat(0)`, a player
    /// who left to none, so what was routed to them presents nowhere.
    pub fn drain_landings_resolving_seats(
        &self,
        data_registry: &DataRegistry,
        sequence_registry: &SequencedPrimitiveRegistry,
        reaction_registry: &ReactionPrimitiveRegistry,
        system_registry: &SystemReactionRegistry,
        script_ctx: &ScriptCtx,
        resolve_seat: &dyn Fn(PlayerId, &EntityRegistry) -> Option<Seat>,
    ) {
        let client = self.role() == SchedulerRole::Client;
        // One landing at a time. A landing's re-parks and `fire` children
        // already past their due tick queue as they enroll and land in this
        // same drain, in order with the rest. Each falls due after its parent
        // and no later than its clock's present, and the chain depth is
        // capped, so the loop ends.
        while let Some(landing) = self.take_next_landing() {
            self.drain_one(
                landing,
                client,
                resolve_seat,
                data_registry,
                sequence_registry,
                reaction_registry,
                system_registry,
                script_ctx,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn drain_one(
        &self,
        landing: Landing,
        client: bool,
        resolve_seat: &dyn Fn(PlayerId, &EntityRegistry) -> Option<Seat>,
        data_registry: &DataRegistry,
        sequence_registry: &SequencedPrimitiveRegistry,
        reaction_registry: &ReactionPrimitiveRegistry,
        system_registry: &SystemReactionRegistry,
        script_ctx: &ScriptCtx,
    ) {
        let _depth = self.begin_enrollment_depth(landing.depth);
        let (address, body_ordinal, class, subject) = landing.key.clone();
        let (origin, host_capture, client_landing) = if client {
            let mode = if self.is_late(landing.due) {
                ClientLanding::Converge
            } else {
                ClientLanding::Land
            };
            (landing.origin.clone(), false, Some(mode))
        } else {
            let origin = match &landing.origin {
                FireOrigin::HostOnly(fired) => FireOrigin::HostOnly(subject_at_landing(
                    fired,
                    subject.map(|(_, player)| player),
                    &script_ctx.registry.borrow(),
                    resolve_seat,
                )),
                every_machine => every_machine.clone(),
            };
            (origin, class != TailClass::OwnState, None)
        };
        let _anchor = self.begin_anchor(landing.due);
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
                    address.clone(),
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
        if let Some((owner, step)) = script_ctx.landing.first_dropped() {
            self.report_client_drop(&address, body_ordinal, class, &owner, &step);
        }
    }

    /// A client drops what only the host lands. Only an own-state landing's
    /// drop is the author's to hear about: nothing replicates the step to
    /// this machine, so it ran nowhere for this player. Once per landing
    /// reaction body until level teardown, suspend, hot reload or a role
    /// change; every other drop is routine and stays at debug. `owner` is the
    /// reaction whose step dropped: the landing's own, or a `fire` target's.
    fn report_client_drop(
        &self,
        address: &str,
        body_ordinal: usize,
        class: TailClass,
        owner: &str,
        step: &str,
    ) {
        let via = if owner == address {
            String::new()
        } else {
            format!(" (via `fire({owner})`)")
        };
        let first = class == TailClass::OwnState
            && self
                .state
                .borrow_mut()
                .warned_own_state_drops
                .insert((address.to_string(), body_ordinal));
        if first {
            log::warn!(
                "[Scheduler] reaction `{address}`{via}: step `{step}` after a `wait` lands only on the host, and this own-state tail fires for this machine's player; it is dropped here"
            );
        } else {
            log::debug!(
                "[Scheduler] reaction `{address}`{via}: client landing did not apply `{step}`"
            );
        }
    }
}

/// The subject a host-only landing routes to: the one its fire routed to,
/// with the seat that player holds now. A subject seated at fire whose seat
/// is gone has departed. A wait drops occupants with every other subject
/// token, so a trigger landing carries none.
fn subject_at_landing(
    fired: &FireSubject,
    player: Option<PlayerId>,
    registry: &EntityRegistry,
    resolve_seat: &dyn Fn(PlayerId, &EntityRegistry) -> Option<Seat>,
) -> FireSubject {
    let seat_now = |seated_at_fire: Option<Seat>| -> Result<Option<Seat>, ()> {
        let Some(seated) = seated_at_fire else {
            return Ok(None);
        };
        let Some(player) = player else {
            return Ok(Some(seated));
        };
        if let Some(seat) = resolve_seat(player, registry) {
            return Ok(Some(seat));
        }
        // A harness with no seat table binds its local pawn to no seat.
        match player {
            PlayerId::Local(pawn) if registry.local_player_pawn() == Some(pawn) => Ok(Some(seated)),
            _ => Err(()),
        }
    };
    match fired {
        FireSubject::None | FireSubject::Departed { .. } => fired.clone(),
        FireSubject::Trigger {
            player: seated,
            trigger,
            ..
        } => match seat_now(*seated) {
            Ok(player) => FireSubject::Trigger {
                player,
                trigger: *trigger,
                occupants: Vec::new(),
            },
            Err(()) => FireSubject::Departed {
                sounds_everywhere: true,
            },
        },
        FireSubject::Teleport {
            player: seated,
            teleporter,
        } => match seat_now(*seated) {
            Ok(player) => FireSubject::Teleport {
                player,
                teleporter: *teleporter,
            },
            Err(()) => FireSubject::Departed {
                sounds_everywhere: true,
            },
        },
        FireSubject::PlayerEvent { player: seated } => match seat_now(*seated) {
            Ok(player) => FireSubject::PlayerEvent { player },
            Err(()) => FireSubject::Departed {
                sounds_everywhere: false,
            },
        },
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

/// Restores the scheduler's previous `current_anchor` when one landing's drain
/// finishes. Holds a borrow of the scheduler struct, not a `RefCell` borrow,
/// like the guards above.
pub(super) struct AnchorGuard<'a> {
    scheduler: &'a ReactionScheduler,
    previous: Option<DueTick>,
}

impl Drop for AnchorGuard<'_> {
    fn drop(&mut self) {
        self.scheduler.state.borrow_mut().current_anchor = self.previous;
    }
}
