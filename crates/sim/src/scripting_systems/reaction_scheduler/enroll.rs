// Enrollment for the `wait` scheduler: the control primitives a reaction's
// `wait` reaches, the caps that bound parked tails, and how each tail parks
// under its source's class and clock.
// See: context/lib/scripting.md §12 (Steps after a `wait`)

use super::*;

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

    sequence_registry.register_control(
        "wait",
        move |script_ctx, address, body_ordinal, tail, args| {
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
            // sourceless. Re-deriving here would be wrong — see O54/O55/O57. The
            // source class and landing origin come from the fire context the tail
            // was reached in, which a `fire`-seeded child inherits through its
            // follow-up origin.
            scheduler.enroll_fired(
                script_ctx,
                address,
                body_ordinal,
                tail,
                ticks,
                interruptible,
            );
        },
    );
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

impl ReactionScheduler {
    /// Enroll the tail a `wait` step reached, under the source class and landing
    /// origin of the fire context it was reached in. On a client, a
    /// host-timeline or host-mirrored tail that holds no machine-local step
    /// and no `fire` parks nothing: every step in it reaches the client by
    /// replication. An own-state tail parks whatever it holds, so its landing
    /// can report what it drops.
    pub(crate) fn enroll_fired(
        &self,
        script_ctx: &ScriptCtx,
        address: &str,
        body_ordinal: usize,
        tail: &[SequenceStep],
        ticks: u32,
        interruptible: bool,
    ) {
        let origin = script_ctx.system_commands.fire_context().origin;
        if self.role() == SchedulerRole::Client
            && TailClass::of(&origin) != TailClass::OwnState
            && !client::tail_has_landing_work(tail, &script_ctx.slot_table.borrow())
        {
            log::debug!(
                "[Scheduler] reaction `{address}`: client tail has no machine-local step; the host lands it"
            );
            return;
        }
        let subject = self.effective_origin(address, body_ordinal);
        self.enroll_with_origin(
            address,
            body_ordinal,
            subject,
            origin,
            tail.to_vec(),
            ticks,
            interruptible,
        );
    }

    /// Test and harness entry point; production enrolls through the `wait`
    /// control handler (`enroll_fired`). Enroll a host-only tail whose
    /// landing dispatches under `HostOnly(FireSubject::None)`, keyed to
    /// `subject` when one is given. `ticks` is the already-converted
    /// whole-tick countdown (>= 1).
    pub fn enroll(
        &self,
        address: &str,
        body_ordinal: usize,
        subject: Option<(EntityId, PlayerId)>,
        tail: Vec<SequenceStep>,
        ticks: u32,
        interruptible: bool,
    ) {
        self.enroll_with_origin(
            address,
            body_ordinal,
            subject,
            FireOrigin::HostOnly(FireSubject::None),
            tail,
            ticks,
            interruptible,
        );
    }

    /// Park `tail` under its instance key for `ticks`, applying the same-key
    /// re-fire rule, the chain-depth bound and the instance cap. A scheduler
    /// with no stated role refuses here rather than at a call site, because
    /// named fires run on every machine.
    #[allow(clippy::too_many_arguments)]
    fn enroll_with_origin(
        &self,
        address: &str,
        body_ordinal: usize,
        subject: Option<(EntityId, PlayerId)>,
        fire_origin: FireOrigin,
        tail: Vec<SequenceStep>,
        ticks: u32,
        interruptible: bool,
    ) {
        let class = TailClass::of(&fire_origin);
        let mut state = self.state.borrow_mut();
        let client = state.role == SchedulerRole::Client;
        if client && class == TailClass::HostOnly {
            // A host-only source does not fire on a client; a tail reached here
            // anyway belongs to the host.
            log::debug!(
                "[Scheduler] reaction `{address}`: host-only tail does not park on a client"
            );
            return;
        }
        // A client never sees the trigger Exit that would cancel a tail, so an
        // interruptible wait lands as non-interruptible there, silently: every
        // client would otherwise warn for a body the host handles correctly.
        let interruptible = interruptible && !client;
        if state.role == SchedulerRole::Disabled {
            // O24/O43: a scheduler with no stated role (harnesses only) parks
            // nothing.
            if state
                .warned_disabled_reactions
                .insert((address.to_string(), body_ordinal))
            {
                log::warn!(
                    "[Scheduler] wait enrollment refused for reaction `{address}` (scheduler disabled); the tail will not run here"
                );
            }
            return;
        }
        let key: InstanceKey = (address.to_string(), body_ordinal, class, subject);
        // O55: a sourceless instance (a `fire`-seeded child, or a batch-seeded
        // enrollment outside the origin guard) has no Exit to cancel on, so an
        // `interruptible` wait is demoted to non-interruptible and warned once at
        // enrollment. V3 makes the install-time version of this decision; this is
        // the runtime path V3's rule already covers.
        let interruptible = if interruptible && subject.is_none() {
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
        // paired enter is still standing. During the residual drain `subject`
        // equals `current_origin`; a player who entered and left within the frame
        // is no longer standing, so the instance must not park — otherwise the
        // beat fires with nobody on the plate and no cancel ever arrives.
        if interruptible
            && subject.is_some()
            && subject == state.current_origin
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
                    .find(|queued| queued.landing.key == key)
                    .map(|queued| queued.interruptible)
                {
                    Some(true) => {
                        state.landings.retain(|queued| queued.landing.key != key);
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
        // A client's tail paths log nothing above debug but the own-state
        // warn-once: the host runs the same chain and warns for it there.
        let drop_level = if client {
            log::Level::Debug
        } else {
            log::Level::Warn
        };
        if depth > MAX_REACTION_CHAIN_DEPTH {
            log::log!(
                drop_level,
                "[Scheduler] reaction `{address}` reached the {MAX_REACTION_CHAIN_DEPTH}-deep enrolled-by chain cap; dropping this enrollment (its pre-wait steps already ran)"
            );
            return;
        }
        let occupied = state.instances.len()
            + state
                .landings
                .iter()
                .filter(|queued| queued.overdue)
                .count();
        if !cap_exempt && occupied >= MAX_PENDING_REACTION_INSTANCES {
            log::log!(
                drop_level,
                "[Scheduler] wait enrollment for `{address}` exceeds the per-level cap of {MAX_PENDING_REACTION_INSTANCES} concurrent instances; dropping it (its pre-wait steps already ran and the tail is abandoned)"
            );
            return;
        }
        let id = state.next_instance_id;
        state.next_instance_id += 1;
        let enrolled_frame = state.frame_counter;
        let ticks = ticks.max(1);
        let clock = if client && class == TailClass::HostTimeline {
            TickClock::Host
        } else {
            TickClock::Local
        };
        // A landing drains at frame end, ticks after it fell due. Everything
        // its drain enrolls counts from that due tick on the same clock, so
        // each segment of a chain falls due at its parent's due tick plus its
        // wait — on the host as on a client's host-timeline mirror, whatever
        // the ticks per frame. A child whose due tick has already passed
        // queues at once and lands in this same drain, in order with the rest.
        let anchored = state.current_anchor.filter(|anchor| anchor.clock == clock);
        let countdown = match (anchored, clock) {
            (Some(anchor), _) => Countdown::Due(DueTick {
                clock,
                tick: anchor.tick.saturating_add(u64::from(ticks)),
            }),
            // A host-timeline tail outside a landing is a `levelLoad` fire:
            // it counts from the level's host `script_time` 0.
            (None, TickClock::Host) => Countdown::Due(DueTick {
                clock,
                tick: u64::from(ticks),
            }),
            (None, TickClock::Local) => Countdown::Ticks(ticks),
        };
        if let (Some(_), Countdown::Due(due)) = (anchored, countdown)
            && state.now(due.clock).is_some_and(|now| due.tick <= now)
        {
            state.landings.push(QueuedLanding {
                id,
                interruptible,
                overdue: true,
                landing: Landing {
                    key,
                    depth,
                    tail,
                    origin: fire_origin,
                    due,
                },
            });
            return;
        }
        state.instances.insert(
            key,
            PendingInstance {
                id,
                tail,
                countdown,
                origin: fire_origin,
                interruptible,
                enrolled_frame,
                depth,
            },
        );
    }
}
