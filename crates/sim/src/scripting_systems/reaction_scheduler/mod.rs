// Scheduler for `wait` tails (E18): the host lands every step, a client only
// its machine-local ones.
// See: context/lib/scripting.md §12 (Steps after a `wait`)

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use postretro_entities::{EntityId, FireOrigin, FireSubject, MachineSource, ScriptCtx};
use postretro_scripting_core::data_descriptors::SequenceStep;
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

use crate::trigger_system::PlayerId;

/// Instance key: `(reaction address, body ordinal, source class, subject)`. The
/// address is the reaction *name* — stable across a `recompose_active_sets`
/// where vector positions are not. The body ordinal keeps two same-named bodies
/// from cancelling each other. The class keeps one source's fire from absorbing
/// another's beat. The subject is `None` for sourceless (named / `levelLoad` /
/// `fire`-seeded) enrollments and `(trigger, player)` for a trigger fire.
pub type InstanceKey = (String, usize, TailClass, Option<(EntityId, PlayerId)>);

/// A tail's source class and clock (`scripting.md` §12, Steps after a `wait`):
/// host-only sources park on the host; the rest park on every machine their
/// source fires on, `levelLoad` on the host timeline and the others on each
/// machine's own clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TailClass {
    HostOnly,
    HostTimeline,
    HostMirrored,
    OwnState,
}

impl TailClass {
    pub fn of(origin: &FireOrigin) -> Self {
        match origin {
            FireOrigin::HostOnly(_) => Self::HostOnly,
            FireOrigin::EveryMachine(MachineSource::HostTimeline) => Self::HostTimeline,
            FireOrigin::EveryMachine(MachineSource::HostMirrored) => Self::HostMirrored,
            FireOrigin::EveryMachine(MachineSource::OwnState) => Self::OwnState,
        }
    }
}

/// Which machine this scheduler runs for. `Disabled` refuses every
/// enrollment with a warning; it is the default only for harnesses that never
/// state a role.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SchedulerRole {
    #[default]
    Disabled,
    /// Host or single player: every tail parks and lands every step.
    Host,
    /// Connected client: tails of every-machine sources park and land only
    /// their machine-local steps.
    Client,
}

/// The tick count a due tick reads. Ticks on different clocks never compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickClock {
    /// This machine's fixed ticks, counted by `evaluate`: every tail on the
    /// host, whose own count is the host timeline, and a client's own-clock
    /// tails.
    Local,
    /// The host timeline as a client reads it: the host's fixed ticks this
    /// level ([`ReactionScheduler::level_tick`] on the host), carried by every
    /// snapshot and extrapolated a bounded way past the newest. A client's
    /// host-timeline tails.
    Host,
}

/// The tick a segment falls due on, and the clock that tick counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueTick {
    pub clock: TickClock,
    pub tick: u64,
}

/// When a parked instance lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Countdown {
    /// Whole local ticks still to elapse (>= 1), for an enrollment outside a
    /// landing's drain. It never advances in the frame that enrolled it
    /// (O12), so its due tick is the first tick past that frame plus the
    /// wait, known only as it counts down; expiry stamps it.
    Ticks(u32),
    /// A fixed due tick: an enrollment reached during a landing's drain,
    /// counted from that landing's due tick, or a client's host-timeline
    /// tail, counted from the host's level tick 0.
    Due(DueTick),
}

#[derive(Debug, Clone)]
struct PendingInstance {
    /// Monotonic id assigned at enrollment; the landing order among landings
    /// due together (O8, O25).
    id: u64,
    /// The remainder of the reaction body after the wait step.
    tail: Vec<SequenceStep>,
    countdown: Countdown,
    /// The fire origin the tail was reached under; the landing dispatches in it.
    origin: FireOrigin,
    /// Whether the paired trigger Exit cancels this instance. Reflects the wait
    /// currently parked at, so in a multi-wait body the parked-at wait governs
    /// both Exit-cancel and re-fire (O17/O18).
    interruptible: bool,
    /// The monotonic frame counter at enrollment. `evaluate` advances no
    /// whole-tick countdown, and `advance_host_clock` expires no host-timeline
    /// instance, whose stamp equals the current frame, so an instance never
    /// advances in the frame that enrolled it — identically for every dispatch
    /// phase.
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
    pub origin: FireOrigin,
    /// The tick this landing fell due on, on its clock. A wait or `fire`
    /// child reached in the landing counts from it, never from the drain.
    pub due: DueTick,
}

/// An expired instance in the landing queue. A queued-but-unrun landing is
/// still cancellable by a paired Exit that arrives between expiry and the
/// drain (O53), so it keeps its interruptible flag.
#[derive(Debug, Clone)]
struct QueuedLanding {
    id: u64,
    interruptible: bool,
    /// Enrolled during a landing's drain already past its due tick, so it
    /// queued without parking. It holds an instance slot until it lands, as
    /// it would have parked had the frame held fewer ticks.
    overdue: bool,
    landing: Landing,
}

#[derive(Debug, Default)]
struct SchedulerState {
    role: SchedulerRole,
    /// Monotonic frame counter — advanced by `begin_frame`, never reset. Distinct
    /// from `tick_index`, which resets every frame.
    frame_counter: u64,
    /// Fixed ticks this machine has evaluated — advanced by `evaluate` on
    /// either role, never reset. A [`TickClock::Local`] due tick reads it.
    tick_counter: u64,
    /// `tick_counter` when this level's clock last reset, so the level tick
    /// counts `evaluate` calls since level install: on the host the
    /// host-timeline clock it replicates.
    level_start_tick: u64,
    /// Monotonic instance-id source.
    next_instance_id: u64,
    instances: BTreeMap<InstanceKey, PendingInstance>,
    /// Tails past their due tick, awaiting the frame-end landing drain, which
    /// resumes them in [`SchedulerState::next_landing`] order.
    landings: Vec<QueuedLanding>,
    /// Client only: the host tick of the latest host-clock reading this level.
    host_tick_now: Option<u64>,
    /// Client only: the host level tick this client revealed at, fixed at its
    /// first host-clock reading as that reading less the client's level clock
    /// (unclamped time since reveal) in ticks. A host-timeline segment due
    /// before it belongs to a late joiner, which converges state instead of
    /// replaying presentation.
    join_host_tick: Option<u64>,
    /// The due tick of the landing draining now ([`Landing::due`]). Every
    /// enrollment reached synchronously in that landing's drain — a nested
    /// re-park in its tail, a `fire` child in its follow-ups, any `wait` they
    /// reach — counts from it on the same clock, never from the drain.
    current_anchor: Option<DueTick>,
    /// Origin of the trigger, teleport or player-event residual currently
    /// being drained, set by the scoped `OriginGuard` that
    /// `crate::residual_drain` and `crate::teleporter_system` hold for one
    /// residual ONLY (released before the deferred batch — O54). A `wait`
    /// reached while it is live keys its instance to this `(source, player)`;
    /// every batch-seeded fire is sourceless.
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
    /// Per-level diagnostic latch for disabled-scheduler enrollment refusal. A
    /// reaction can fire repeatedly in a frame, but the lost tail is
    /// actionable only once per reaction until level lifecycle state clears.
    warned_disabled_reactions: BTreeSet<(String, usize)>,
    /// Per-level diagnostic latch for an interruptible wait reached without a
    /// trigger origin. The wait is demoted on every enrollment, but the warning
    /// is emitted once per reaction until level lifecycle state clears.
    warned_sourceless_interruptible_reactions: BTreeSet<(String, usize)>,
    /// Client only: own-state reaction bodies whose landing already warned
    /// that it dropped a host-only step, until level lifecycle state clears.
    warned_own_state_drops: BTreeSet<(String, usize)>,
}

impl SchedulerState {
    /// Move expired instances into the landing queue, stamping each landing
    /// with the tick it fell due on. A fixed due tick keeps its own clock; a
    /// whole-tick countdown expires on the local tick that zeroed it, so it
    /// is stamped with that tick on [`TickClock::Local`].
    fn expire(&mut self, keys: Vec<InstanceKey>) {
        for key in keys {
            if let Some(instance) = self.instances.remove(&key) {
                let due = match instance.countdown {
                    Countdown::Due(due) => due,
                    Countdown::Ticks(_) => DueTick {
                        clock: TickClock::Local,
                        tick: self.tick_counter,
                    },
                };
                self.landings.push(QueuedLanding {
                    id: instance.id,
                    interruptible: instance.interruptible,
                    overdue: false,
                    landing: Landing {
                        key,
                        depth: instance.depth,
                        tail: instance.tail,
                        origin: instance.origin,
                        due,
                    },
                });
            }
        }
    }

    /// The latest tick `clock` has reached on this machine: the local tick
    /// count, or a client's latest host-clock reading.
    fn now(&self, clock: TickClock) -> Option<u64> {
        match clock {
            TickClock::Local => Some(self.tick_counter),
            TickClock::Host => self.host_tick_now,
        }
    }

    /// Index of the queued landing to resume next. The drain takes one at a
    /// time, so a re-fire meanwhile dedups against every landing still
    /// queued, and a landing's overdue re-parks and `fire` children, queued
    /// as they enroll, take their place in this order.
    ///
    /// - On the host every due tick is local: earliest due tick first, then
    ///   ascending id (O8, O25), so a frame of several ticks lands on the
    ///   tick timeline.
    /// - On a client, host-timeline landings due before the current reading
    ///   tick are catch-up of the host's past and land first, in due order.
    ///   The rest — host-timeline landings due on the reading tick and every
    ///   own-clock landing — are this frame's present and land together in
    ///   ascending id, the enrollment order: own ticks and host ticks never
    ///   compare, and enrollment is the order both clocks share.
    fn next_landing(&self) -> Option<usize> {
        let host = self.role == SchedulerRole::Host;
        let order = |queued: &QueuedLanding| {
            let due = queued.landing.due;
            let catch_up = due.clock == TickClock::Host
                && self.host_tick_now.is_some_and(|now| due.tick < now);
            if catch_up {
                (0, due.tick, queued.id)
            } else if host {
                (1, due.tick, queued.id)
            } else {
                (1, 0, queued.id)
            }
        };
        self.landings
            .iter()
            .enumerate()
            .min_by_key(|(_, queued)| order(queued))
            .map(|(index, _)| index)
    }

    /// Restart the level clock (level teardown, suspend, role change): the
    /// level tick counts from here, and the next level's first reading fixes
    /// its join tick afresh. A hot reload keeps the level, so it keeps this.
    fn reset_level_clock(&mut self) {
        self.level_start_tick = self.tick_counter;
        self.host_tick_now = None;
        self.join_host_tick = None;
        self.current_anchor = None;
    }

    /// Fixed ticks evaluated since this level's clock reset.
    fn level_tick(&self) -> u64 {
        self.tick_counter - self.level_start_tick
    }

    /// Drop every parked and queued tail and the per-level warn latches,
    /// logging the count at the role's level.
    fn drop_tails(&mut self) {
        let dropped = self.instances.len() + self.landings.len();
        if dropped > 0 {
            if self.role == SchedulerRole::Client {
                log::debug!(
                    "[Scheduler] dropping {dropped} client wait tail(s) on level teardown / suspend / hot reload"
                );
            } else {
                log::warn!(
                    "[Scheduler] dropping {dropped} pending timed-reaction instance(s) on level teardown / suspend / hot reload; a half-completed beat will not resume"
                );
            }
        }
        self.instances.clear();
        self.landings.clear();
        self.warned_disabled_reactions.clear();
        self.warned_sourceless_interruptible_reactions.clear();
        self.warned_own_state_drops.clear();
    }
}

/// Cloneable session-owned handle to the timed-reaction scheduler. A `wait`
/// control step inside a `sequence` reaction body enrolls the remainder of
/// that body here and stops the synchronous drain; the scheduler counts ticks
/// and, on expiry, moves the stored tail into a landing queue that the
/// frame-end drain resumes through the shipped residual path.
///
/// Shaped exactly like `MoverAutoCloseTimers`: `Rc<RefCell<_>>`, main-thread only,
/// owned on the session beside `slot_accumulator_bindings` and cloned into the
/// control handler at registration. It does not participate in snapshots or
/// digests. On a connected client it parks every-machine tails only: an
/// own-state tail always, a host-mirrored or host-timeline tail only when it
/// holds something the client can land. It lands only their machine-local
/// steps.
#[derive(Debug, Clone, Default)]
pub struct ReactionScheduler {
    state: Rc<RefCell<SchedulerState>>,
}

impl ReactionScheduler {
    /// Set the machine this scheduler runs for. A role change drops every
    /// parked tail: what parked under one role means nothing under another.
    pub fn set_role(&self, role: SchedulerRole) {
        let mut state = self.state.borrow_mut();
        if state.role == role {
            return;
        }
        state.role = role;
        state.instances.clear();
        state.landings.clear();
        state.reset_level_clock();
        state.warned_disabled_reactions.clear();
        state.warned_sourceless_interruptible_reactions.clear();
        state.warned_own_state_drops.clear();
    }

    pub fn role(&self) -> SchedulerRole {
        self.state.borrow().role
    }

    /// Drop all transient level state without changing the session role, and
    /// restart the level clock. Wired into `clear_surface_lifetime_level_state`
    /// (level teardown and the suspend path). A parked instance holds a
    /// snapshot of a body that the next level or a suspend may invalidate, so
    /// a half-completed beat is dropped rather than replayed — the `warn!`
    /// naming the count keeps that distinguishable from a bug (O38, O39). A
    /// client's dropped tails log at debug: the host warns for the beats it
    /// owns, and a client's own-state beat ends with the level it belongs to.
    pub fn clear(&self) {
        let mut state = self.state.borrow_mut();
        state.drop_tails();
        state.reset_level_clock();
    }

    /// The staged-manifest hot-reload commit's [`Self::clear`]: an author edit
    /// may invalidate a parked body, so every tail drops (O40), but the level
    /// stays installed and `levelLoad` does not re-fire, so the level clock
    /// keeps counting. The host keeps replicating the same timeline, and a
    /// client keeps the host tick it revealed at.
    pub fn clear_for_hot_reload(&self) {
        self.state.borrow_mut().drop_tails();
    }

    /// Fixed ticks `evaluate` has counted since this level's clock reset at
    /// teardown or role change. A `levelLoad` tail enrolled at install with
    /// `wait(T)` lands on the host at level tick `T`, so the host replicates
    /// this as the host timeline (`SnapshotMessage::level_tick`), read after
    /// the frame's ticks and landings.
    pub fn level_tick(&self) -> u64 {
        self.state.borrow().level_tick()
    }

    /// Advance the monotonic frame counter once per frame. Three call sites: the
    /// `RedrawRequested` arm, `SimHarness::frame`, and `run_headless_inner`'s
    /// loop. A counter advanced only by window events never advances in the
    /// headless drivers, where every instance would then be skipped forever.
    pub fn begin_frame(&self) {
        self.state.borrow_mut().frame_counter += 1;
    }

    /// Set the origin for the trigger, teleport or player-event residual
    /// currently being drained, cleared on drop. Held across that residual
    /// ONLY (released before the deferred batch — O54). `standing` is whether
    /// this origin's paired enter is live at drain time, feeding the O52
    /// enrollment check; a teleport or player event has no paired enter and
    /// passes `false`, a player event keyed by the event player's pawn.
    /// Mirrors `begin_resume`.
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
            return resume_key.3;
        }
        None
    }

    /// Advance this machine's local clock by one fixed tick. A whole-tick
    /// countdown advances, skipping any enrolled this very frame (O12); a
    /// local due tick expires when the clock reaches it. Expired instances
    /// move to the landing queue for the frame-end drain (O8/O25).
    /// Countdowns are whole ticks, so this takes no `dt`. The host calls it
    /// once per fixed tick, a connected client once per client tick.
    ///
    /// `exit_fires` is this tick's paired-trigger Exit set (`(trigger, player)`).
    /// Cancels apply **before** the countdown advance so an Exit on the exact
    /// landing tick wins (O4); they reach the landing queue too, so an Exit
    /// between an instance's expiry and the frame-end drain still cancels (O53).
    pub fn evaluate(&self, exit_fires: &[(EntityId, PlayerId)]) {
        let mut state = self.state.borrow_mut();
        if state.role == SchedulerRole::Disabled {
            return;
        }
        state.tick_counter += 1;
        let now = state.tick_counter;
        // O4/O53: cancel matching interruptible instances (and queued landings)
        // BEFORE advancing countdowns. A non-interruptible instance ignores its
        // paired Exit (O5), so the flag gates the cancel.
        if !exit_fires.is_empty() {
            state.instances.retain(|key, instance| {
                !(instance.interruptible
                    && key.3.is_some_and(|origin| exit_fires.contains(&origin)))
            });
            state.landings.retain(|queued| {
                !(queued.interruptible
                    && queued
                        .landing
                        .key
                        .3
                        .is_some_and(|origin| exit_fires.contains(&origin)))
            });
        }
        let current = state.frame_counter;
        let mut expired: Vec<InstanceKey> = Vec::new();
        for (key, instance) in state.instances.iter_mut() {
            match &mut instance.countdown {
                Countdown::Ticks(remaining) => {
                    if instance.enrolled_frame == current {
                        continue;
                    }
                    *remaining = remaining.saturating_sub(1);
                    if *remaining == 0 {
                        expired.push(key.clone());
                    }
                }
                // Enrolled during a landing's drain, which ends the frame, so
                // the frame skip is moot: the due tick is absolute.
                Countdown::Due(due) if due.clock == TickClock::Local && due.tick <= now => {
                    expired.push(key.clone());
                }
                // Not yet due, or a client's host-timeline instance, which
                // waits for the host clock (`advance_host_clock`).
                Countdown::Due(_) => {}
            }
        }
        // No per-tick sort here: the drain picks each landing in order
        // (`SchedulerState::next_landing`), so sorting first is dead work.
        state.expire(expired);
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
        if state.role == SchedulerRole::Disabled {
            return;
        }
        state.instances.retain(|key, instance| {
            let orphaned = instance.interruptible
                && key.3.is_some_and(|origin| !standing_enters.contains(&origin));
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
        state.landings.retain(|queued| {
            !(queued.interruptible
                && queued
                    .landing
                    .key
                    .3
                    .is_some_and(|origin| !standing_enters.contains(&origin)))
        });
    }

    /// Take the next landing to drain, in [`SchedulerState::next_landing`]
    /// order, leaving the rest queued.
    fn take_next_landing(&self) -> Option<Landing> {
        let mut state = self.state.borrow_mut();
        let index = state.next_landing()?;
        Some(state.landings.remove(index).landing)
    }

    /// The scheduler's own unit tests take the queue, in landing order,
    /// without resuming it; production resumes through
    /// [`Self::drain_landings`].
    #[cfg(test)]
    pub(crate) fn take_landings(&self) -> Vec<Landing> {
        std::iter::from_fn(|| self.take_next_landing()).collect()
    }

    /// Number of parked (not-yet-landed) instances (test observability).
    #[cfg(any(test, feature = "test-support"))]
    pub fn pending_len(&self) -> usize {
        self.state.borrow().instances.len()
    }

    /// The host tick a client fixed as this level's join (test observability);
    /// `None` until the level's first host reading, and after a clock reset.
    #[cfg(any(test, feature = "test-support"))]
    pub fn join_host_tick(&self) -> Option<u64> {
        self.state.borrow().join_host_tick
    }
}

/// Clears the scheduler's `current_origin` when one residual's drain
/// finishes. Held for one residual by `crate::residual_drain` (trigger and
/// player-event residuals) and `crate::teleporter_system` (teleport
/// residuals), so a `wait` reached synchronously through
/// `fire_prepartitioned_reactions_with_sequences` keys to this origin, and
/// released before the deferred batch runs so a batch-seeded `fire` is
/// sourceless (O54). Owns a scheduler clone (an `Rc` clone), so it does not
/// hold a borrow of the session while the residual runs.
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

mod client;
mod enroll;
mod landing;

pub(crate) use enroll::ms_to_ticks;
pub use enroll::register_reaction_control_primitives;
#[cfg(test)]
pub(crate) use enroll::{MAX_PENDING_REACTION_INSTANCES, MAX_REACTION_CHAIN_DEPTH};

#[cfg(test)]
mod client_tests;
#[cfg(test)]
mod landing_origin_tests;
#[cfg(test)]
mod tests;
