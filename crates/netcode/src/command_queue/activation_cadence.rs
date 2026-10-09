// Client-domain recovery admission for remote activation starts.
// See: context/lib/networking.md §Combat authority · §Host input command queue

use crate::prediction::client_tick_le;
use postretro_entities::EntityId;
use postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY;
use postretro_foundation::ActivationToken;

/// 150 ms at 60 Hz: the charge rule's tolerance, applied to cadence.
pub(crate) const CADENCE_TOLERANCE_TICKS: u32 =
    postretro_combat_model::activation::CHARGE_TOLERANCE_TICKS;

/// 500 ms at 60 Hz. A start may claim a host time this far behind host now and
/// still be credited there, so a stall's backlog drains at once. Credited no
/// earlier than host now, a held trigger's backlog drained at one shot per
/// recovery, the rate new starts arrive, so the rest of the hold lagged by the
/// whole stall. A start claiming further back is refused: past this, a late
/// shot costs more than a hold that stays on time.
pub(crate) const CATCH_UP_ALLOWANCE_TICKS: u32 = 30;

/// Most executions one weapon can admit in any `window` host ticks, whatever
/// client ticks are stamped. A zero recovery imposes no spacing; playout's one
/// start per host tick bounds it instead, well inside this.
pub fn window_bound(window: u32, recovery_ticks: u32) -> u32 {
    (window + CATCH_UP_ALLOWANCE_TICKS + CADENCE_TOLERANCE_TICKS) / recovery_ticks.max(1) + 1
}

/// What the client half of the rule says about a start bound to a weapon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CadenceVerdict {
    /// No recovery recorded for this weapon; the host's own cooldown applies.
    Unrecorded,
    /// Client spacing covers the recorded recovery.
    Eligible,
    /// The start claims a client tick inside the recorded recovery.
    Refused,
}

#[derive(Debug, Clone, Copy)]
struct Recovery {
    weapon: EntityId,
    /// Client tick at which the recovery began.
    client_tick: u32,
    /// Host tick credited to that same moment.
    credit_tick: u32,
    /// `ceil(recovery_ms / tick_ms)`, the client's predicted countdown in ticks.
    recovery_ticks: u32,
    /// Host ticks since credit on which a real command named another firing
    /// slot while this recovery still owed. A holstered weapon's cooldown is
    /// frozen, so those ticks do not count toward its recovery.
    frozen_ticks: u32,
}

/// The delivered start whose execution may begin the next recovery.
#[derive(Debug, Clone, Copy)]
struct Execution {
    token: ActivationToken,
    firing_slot: u8,
    /// Client tick at which execution began: the start, or the release of a charge.
    client_tick: u32,
    /// Host tick at which the host began that same execution step.
    host_tick: u32,
    credit_tick: u32,
    /// First release delivered to this execution: client release tick and the
    /// host tick it was delivered on. Only a charged action's shots wait on it.
    release: Option<(u32, u32)>,
}

impl Recovery {
    /// Host ticks since credit that the weapon was the firing weapon. Negative
    /// while credit still leads host time.
    fn active_ticks(&self, host_tick: u32) -> i64 {
        host_ticks_since(host_tick, self.credit_tick) - i64::from(self.frozen_ticks)
    }

    /// Host ticks after `host_tick` before one recovery has passed since credit.
    fn owed_ticks(&self, host_tick: u32) -> u32 {
        (i64::from(self.recovery_ticks) - self.active_ticks(host_tick)).max(0) as u32
    }

    /// Host tick the client claims for a start at `start_tick`: the credit,
    /// moved by the client's own spacing since the recovery began.
    fn claimed_tick(&self, start_tick: u32) -> u32 {
        self.credit_tick
            .wrapping_add_signed(start_tick.wrapping_sub(self.client_tick) as i32)
    }
}

/// One client's cadence records and the execution that may begin the next one.
///
/// Two tick domains meet here. Client ticks are the client's own stamps: start
/// ticks, release ticks, and the client tick at which a recovery began. Host
/// ticks are this client's playout clock, `ClientCommandState::host_tick`,
/// which advances once per host fixed tick the client is resolved.
///
/// A start is eligible once its client tick is at least the recovery after the
/// client tick its weapon's previous recovery began (the client half, checked
/// when the start binds a weapon), and host time since that recovery's credit,
/// less the ticks the weapon spent holstered, plus `CADENCE_TOLERANCE_TICKS`,
/// covers the recovery (the host half, which holds the start in its lane).
///
/// Each execution is credited at the client's claimed host time, clamped to
/// [host now − `CATCH_UP_ALLOWANCE_TICKS`, host now + `CADENCE_TOLERANCE_TICKS`].
/// An early start carries its lead into the next instead of earning a fresh
/// tolerance; a backlog up to the allowance old drains at once; a start claiming
/// further back is refused (`lags_allowance`). A charged execution fires at its
/// release, so its credit moves to the release, forward only; any other
/// execution ignores releases, as the machine does.
///
/// Records are per weapon: one per firing slot, naming the weapon whose
/// execution began it, so an A→B→A switch finds A's own record. A record stands
/// only while its weapon still holds that slot in the client's inventory;
/// `release_departed` clears it once the weapon leaves (drop, hand-over,
/// despawn) and reports the host ticks its credit still owes, which the host
/// charges to that weapon's own cooldown.
///
/// Bound, per weapon (pinned by the `cannot_exceed_window_bound` tests). The
/// client half makes each claim at least one recovery past the previous credit;
/// the host half admits only once host now + tolerance reaches that point, so
/// the upper clamp never cuts it and each credit is at least one recovery after
/// the last. The first credit in a window is at least its admission minus the
/// allowance, the last at most its admission plus the tolerance. So over any W
/// host ticks one weapon admits at most `window_bound(W, R)` =
/// ⌊(W + allowance + tolerance) / R⌋ + 1 executions, whatever client ticks are
/// stamped. The constant above W / R is 1 + 39 / R: at most 2 for any recovery
/// of 39 ticks (650 ms) or more, about 5.9 at 130 ms. It is a one-time catch-up,
/// never a faster sustained rate. Holstered ticks only make the host half
/// stricter. A departed weapon keeps the chain through its cooldown: the owed
/// ticks hold its next fire to at least one recovery after its last credit.
#[derive(Debug, Default)]
pub(crate) struct ActivationCadence {
    /// Latest recovery per firing slot, each naming the weapon that began it.
    recoveries: [Option<Recovery>; WIELDABLE_SLOT_CAPACITY],
    /// A record another weapon's recovery overwrote before `release_departed`
    /// saw its weapon leave the slot; reported there like any departure.
    displaced: Option<Recovery>,
    execution: Option<Execution>,
}

impl ActivationCadence {
    fn recovery(&self, firing_slot: u8) -> Option<Recovery> {
        self.recoveries
            .get(usize::from(firing_slot))
            .copied()
            .flatten()
    }

    /// Whether any weapon holds a record; none means nothing can depart.
    pub fn has_records(&self) -> bool {
        self.displaced.is_some() || self.recoveries.iter().any(Option::is_some)
    }

    /// Host half of the rule for a start that would fire from `firing_slot`:
    /// the record of the weapon holding that slot.
    pub fn host_time_allows(&self, firing_slot: u8, host_tick: u32) -> bool {
        self.recovery(firing_slot).is_none_or(|recovery| {
            recovery.active_ticks(host_tick) + i64::from(CADENCE_TOLERANCE_TICKS)
                >= i64::from(recovery.recovery_ticks)
        })
    }

    /// A start that would fire from `firing_slot` claims a host time more than
    /// the allowance behind host now, and was stamped more than the allowance
    /// before `newest_client_tick`, the newest command this client has sent.
    /// The second clause keeps a whole stream that trails its old credit
    /// (clock drift, a lasting rise in latency) from reading as a backlog: only
    /// starts stuck behind newer input are refused.
    pub fn lags_allowance(
        &self,
        token: ActivationToken,
        firing_slot: u8,
        host_tick: u32,
        newest_client_tick: u32,
    ) -> bool {
        let allowance = i64::from(CATCH_UP_ALLOWANCE_TICKS);
        self.recovery(firing_slot).is_some_and(|recovery| {
            host_ticks_since(host_tick, recovery.claimed_tick(token.start_tick)) > allowance
                && i64::from(newest_client_tick.wrapping_sub(token.start_tick) as i32) > allowance
        })
    }

    /// A real command named `firing_slot` on `host_tick`. Every other slot's
    /// weapon is holstered for that tick, so a recovery it still owes freezes,
    /// as its own cooldown does. Without this, a switch back would find the
    /// holstered time already counted and fire ahead of the host player.
    ///
    /// `fired_before(slot)`: a start from that slot is retained and already
    /// due, so the client stamped it, wielding that weapon, before this
    /// command. This command's later switch did not holster the weapon for
    /// that start, so its recovery does not freeze; freezing it held the start
    /// behind a backlog that had already switched away.
    pub fn holster_others(
        &mut self,
        firing_slot: u8,
        host_tick: u32,
        fired_before: impl Fn(u8) -> bool,
    ) {
        for (slot, entry) in self.recoveries.iter_mut().enumerate() {
            if slot != usize::from(firing_slot)
                && !u8::try_from(slot).is_ok_and(&fired_before)
                && let Some(recovery) = entry.as_mut()
                && recovery.active_ticks(host_tick) < i64::from(recovery.recovery_ticks)
            {
                recovery.frozen_ticks = recovery.frozen_ticks.saturating_add(1);
            }
        }
    }

    /// Client half of the rule. Wrap-aware: a start at or before the recorded
    /// client tick never spaces past it. The record must name both the weapon
    /// and the slot: the host half is keyed by slot, so a weapon that moved slot
    /// would otherwise escape it, and falls back to the host cooldown instead.
    pub fn client_spacing(
        &self,
        weapon: EntityId,
        firing_slot: u8,
        start_tick: u32,
    ) -> CadenceVerdict {
        match self
            .recovery(firing_slot)
            .filter(|recovery| recovery.weapon == weapon)
        {
            None => CadenceVerdict::Unrecorded,
            Some(recovery)
                if client_tick_le(
                    recovery.client_tick.wrapping_add(recovery.recovery_ticks),
                    start_tick,
                ) =>
            {
                CadenceVerdict::Eligible
            }
            Some(_) => CadenceVerdict::Refused,
        }
    }

    /// A start left the retained lane on `host_tick`. Its credit continues the
    /// recorded recovery by the client's spacing.
    pub fn begin(&mut self, token: ActivationToken, firing_slot: u8, host_tick: u32) {
        let credit_tick = match self.recovery(firing_slot) {
            Some(recovery) => credit(
                recovery.credit_tick,
                token.start_tick.wrapping_sub(recovery.client_tick) as i32,
                host_tick,
            ),
            None => host_tick,
        };
        self.execution = Some(Execution {
            token,
            firing_slot,
            client_tick: token.start_tick,
            host_tick,
            credit_tick,
            release: None,
        });
    }

    /// A release was delivered to `token`'s execution on `host_tick`. It moves
    /// the cadence clock only if that execution's shots began at it, which
    /// `recovery_began` learns from the host machine.
    pub fn release(&mut self, token: ActivationToken, release_tick: u32, host_tick: u32) {
        if let Some(execution) = self
            .execution
            .as_mut()
            .filter(|execution| execution.token == token)
        {
            execution.release.get_or_insert((release_tick, host_tick));
        }
    }

    /// The host machine began `token`'s recovery on `host_tick`. Authored waits
    /// run one per host tick on both peers, so the host offset since execution
    /// began is also the client offset.
    ///
    /// `charged`: the action charges, so its execution began at the client's
    /// release, not its start. Any other execution ignores a release, which the
    /// machine ignores too; moving the clock there would shift a burst's
    /// recovery by however early the release was delivered. A release stamped
    /// before the execution never moves the clock back, so it cannot shed credit.
    pub fn recovery_began(
        &mut self,
        token: ActivationToken,
        weapon: EntityId,
        recovery_ticks: u32,
        charged: bool,
        host_tick: u32,
    ) {
        let Some(execution) = self
            .execution
            .as_mut()
            .filter(|execution| execution.token == token)
        else {
            return;
        };
        if charged && let Some((release_tick, release_host_tick)) = execution.release.take() {
            let span = release_tick.wrapping_sub(execution.client_tick) as i32;
            if span >= 0 {
                execution.credit_tick = credit(execution.credit_tick, span, release_host_tick);
                execution.client_tick = release_tick;
                execution.host_tick = release_host_tick;
            }
        }
        let execution = *execution;
        let Some(entry) = self.recoveries.get_mut(usize::from(execution.firing_slot)) else {
            return;
        };
        if let Some(previous) = entry.filter(|previous| previous.weapon != weapon) {
            self.displaced = Some(previous);
        }
        let offset = host_tick.wrapping_sub(execution.host_tick);
        *entry = Some(Recovery {
            weapon,
            client_tick: execution.client_tick.wrapping_add(offset),
            credit_tick: execution.credit_tick.wrapping_add(offset),
            recovery_ticks,
            frozen_ticks: 0,
        });
    }

    /// Clear every record whose weapon no longer holds its slot, by `held(slot)`.
    /// Yields each departed weapon with the host ticks after `host_tick` that its
    /// credited recovery still owes; a record never outlives its weapon's holding.
    pub fn release_departed(
        &mut self,
        held: impl Fn(usize) -> Option<EntityId>,
        host_tick: u32,
    ) -> impl Iterator<Item = (EntityId, u32)> {
        let mut departed = [None; WIELDABLE_SLOT_CAPACITY];
        for (slot, entry) in self.recoveries.iter_mut().enumerate() {
            if entry.is_some_and(|recovery| held(slot) != Some(recovery.weapon)) {
                departed[slot] = entry.take();
            }
        }
        departed
            .into_iter()
            .chain([self.displaced.take()])
            .flatten()
            .map(move |recovery| (recovery.weapon, recovery.owed_ticks(host_tick)))
    }
}

/// Signed host ticks from `earlier` to `now`; credit may lead host time.
fn host_ticks_since(now: u32, earlier: u32) -> i64 {
    i64::from(now.wrapping_sub(earlier) as i32)
}

/// The client's claimed host tick, clamped to
/// [host now − `CATCH_UP_ALLOWANCE_TICKS`, host now + `CADENCE_TOLERANCE_TICKS`].
fn credit(previous_credit: u32, client_span: i32, host_tick: u32) -> u32 {
    let claimed = previous_credit.wrapping_add_signed(client_span);
    let lead = host_ticks_since(claimed, host_tick).clamp(
        -i64::from(CATCH_UP_ALLOWANCE_TICKS),
        i64::from(CADENCE_TOLERANCE_TICKS),
    );
    host_tick.wrapping_add_signed(lead as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_foundation::ActivationLane;

    const R: u32 = 8;

    fn token(start_tick: u32) -> ActivationToken {
        ActivationToken {
            start_tick,
            lane: ActivationLane::Primary,
        }
    }
    fn weapon() -> EntityId {
        EntityId::from_raw(9)
    }
    /// Admit one single-shot execution the way playout and the host machine do.
    fn admit(cadence: &mut ActivationCadence, start_tick: u32, host_tick: u32) -> bool {
        if !cadence.host_time_allows(0, host_tick)
            || cadence.client_spacing(weapon(), 0, start_tick) == CadenceVerdict::Refused
        {
            return false;
        }
        cadence.begin(token(start_tick), 0, host_tick);
        cadence.recovery_began(token(start_tick), weapon(), R, false, host_tick);
        true
    }

    #[test]
    fn cadence_client_spacing_refuses_inside_recovery_and_admits_at_it() {
        let mut cadence = ActivationCadence::default();
        assert_eq!(
            cadence.client_spacing(weapon(), 0, 100),
            CadenceVerdict::Unrecorded
        );
        assert!(admit(&mut cadence, 100, 0));
        assert_eq!(
            cadence.client_spacing(weapon(), 0, 107),
            CadenceVerdict::Refused
        );
        assert_eq!(
            cadence.client_spacing(weapon(), 0, 108),
            CadenceVerdict::Eligible
        );
        assert_eq!(
            cadence.client_spacing(weapon(), 0, 99),
            CadenceVerdict::Refused,
            "a start before the recorded recovery never spaces past it"
        );
        assert_eq!(
            cadence.client_spacing(EntityId::from_raw(10), 0, 101),
            CadenceVerdict::Unrecorded,
            "another weapon keeps its own host cooldown"
        );
        assert_eq!(
            cadence.client_spacing(weapon(), 1, 108),
            CadenceVerdict::Unrecorded,
            "the weapon in another slot escapes the slot-keyed host half, so it keeps its host cooldown"
        );
    }

    #[test]
    fn cadence_client_spacing_is_wrap_aware() {
        let mut cadence = ActivationCadence::default();
        assert!(admit(&mut cadence, u32::MAX - 3, 0));
        assert_eq!(
            cadence.client_spacing(weapon(), 0, 3),
            CadenceVerdict::Refused,
            "seven client ticks across the wrap stay inside recovery"
        );
        assert_eq!(
            cadence.client_spacing(weapon(), 0, 4),
            CadenceVerdict::Eligible
        );
        assert_eq!(
            cadence.client_spacing(weapon(), 0, u32::MAX - 4),
            CadenceVerdict::Refused
        );
    }

    #[test]
    fn cadence_host_time_holds_a_compressed_start_until_tolerance_covers_it() {
        let mut cadence = ActivationCadence::default();
        assert!(admit(&mut cadence, 100, 50));
        // Client spacing is fine, but a burst of late starts reaches the host on
        // consecutive ticks. The first two fit inside the tolerance.
        assert!(admit(&mut cadence, 108, 51));
        assert!(!cadence.host_time_allows(0, 52), "credit is now 58");
        assert!(cadence.host_time_allows(0, 57));
        assert!(
            cadence.host_time_allows(1, 52),
            "another slot's start waits on nothing"
        );
    }

    #[test]
    fn cadence_backlog_within_the_allowance_drains_at_once_from_its_claims() {
        let mut cadence = ActivationCadence::default();
        assert!(admit(&mut cadence, 100, 0));
        // A stall delivers starts claiming host ticks 8, 16, 24 and 32 together
        // at host tick 30. Each is credited at its claim, so they leave at once
        // rather than one per recovery behind the stall.
        for start in [108, 116, 124, 132] {
            assert!(admit(&mut cadence, start, 30), "start {start}");
        }
        assert!(
            !admit(&mut cadence, 140, 30),
            "a claim past host now + tolerance waits"
        );
        assert!(admit(&mut cadence, 140, 31));
    }

    #[test]
    fn cadence_start_lagging_past_the_allowance_is_refused_only_behind_newer_input() {
        let mut cadence = ActivationCadence::default();
        assert!(admit(&mut cadence, 100, 0));
        let late = token(108);
        // Claims host tick 8: 30 ticks behind at host tick 38, 32 at host tick 40.
        assert!(!cadence.lags_allowance(late, 0, 38, 200));
        assert!(
            cadence.lags_allowance(late, 0, 40, 200),
            "a backlog start stuck behind newer input"
        );
        assert!(
            !cadence.lags_allowance(late, 0, 40, 110),
            "the whole stream trails its old credit: drift, not a backlog"
        );
        assert!(
            !cadence.lags_allowance(late, 1, 40, 200),
            "another slot has no claim to measure"
        );
        // Admitted anyway, it is credited no further back than the allowance:
        // host tick 10, so the drain stops at the window bound.
        assert!(admit(&mut cadence, 108, 40));
        let mut start = 116;
        while admit(&mut cadence, start, 40) {
            start += R;
        }
        assert_eq!(
            (start - 108) / R,
            window_bound(0, R),
            "admissions on host tick 40"
        );
    }

    #[test]
    fn cadence_charge_recovery_runs_from_the_client_release_tick() {
        let mut cadence = ActivationCadence::default();
        assert!(admit(&mut cadence, 100, 0));
        let charged = token(120);
        cadence.begin(charged, 0, 20);
        // The release named client tick 150 reaches the host late, at 60.
        cadence.release(charged, 150, 60);
        cadence.recovery_began(charged, weapon(), R, true, 60);
        assert_eq!(
            cadence.client_spacing(weapon(), 0, 157),
            CadenceVerdict::Refused
        );
        assert_eq!(
            cadence.client_spacing(weapon(), 0, 158),
            CadenceVerdict::Eligible
        );
    }

    #[test]
    fn cadence_charge_release_stamped_before_its_start_never_rewinds_the_clock() {
        let mut cadence = ActivationCadence::default();
        assert!(admit(&mut cadence, 100, 0));
        let charged = token(108);
        // Client spacing claims host tick 8; it starts at 1 and carries that lead.
        cadence.begin(charged, 0, 1);
        cadence.release(charged, 50, 1);
        cadence.recovery_began(charged, weapon(), R, true, 1);
        assert_eq!(
            cadence.client_spacing(weapon(), 0, 115),
            CadenceVerdict::Refused,
            "recovery still runs from the start, not the earlier release"
        );
        assert!(!cadence.host_time_allows(0, 6), "the start's lead is kept");
        assert!(cadence.host_time_allows(0, 7));
    }

    #[test]
    fn cadence_burst_recovery_begins_at_its_last_shot() {
        let mut cadence = ActivationCadence::default();
        cadence.begin(token(100), 0, 10);
        cadence.recovery_began(token(100), weapon(), R, false, 10);
        // A release delivered mid-burst, early or stamped before the start,
        // does not move an uncharged burst's clock: the machine ignores it.
        cadence.release(token(100), 102, 11);
        cadence.recovery_began(token(100), weapon(), R, false, 13);
        cadence.release(token(100), 40, 14);
        cadence.recovery_began(token(100), weapon(), R, false, 16);
        assert_eq!(
            cadence.client_spacing(weapon(), 0, 113),
            CadenceVerdict::Refused
        );
        assert_eq!(
            cadence.client_spacing(weapon(), 0, 114),
            CadenceVerdict::Eligible
        );
    }

    /// Admissions per host window never exceed `window_bound`, whatever client
    /// ticks the client stamps.
    #[test]
    fn cadence_stamping_client_cannot_exceed_window_bound() {
        for recovery_ticks in [1, 4, 8, 9, 30] {
            for stamp_step in [recovery_ticks, recovery_ticks * 3, 1000] {
                let mut cadence = ActivationCadence::default();
                let mut admitted = Vec::new();
                let mut start_tick = u32::MAX - 500;
                // Several attempts per host tick, each stamped one recovery later.
                for host_tick in 0..600u32 {
                    for _ in 0..4 {
                        let host_ok = cadence.host_time_allows(0, host_tick);
                        if host_ok
                            && cadence.client_spacing(weapon(), 0, start_tick)
                                != CadenceVerdict::Refused
                        {
                            cadence.begin(token(start_tick), 0, host_tick);
                            cadence.recovery_began(
                                token(start_tick),
                                weapon(),
                                recovery_ticks,
                                false,
                                host_tick,
                            );
                            admitted.push(host_tick);
                            start_tick = start_tick.wrapping_add(stamp_step);
                        }
                    }
                }
                for (first, &opened) in admitted.iter().enumerate() {
                    for (last, &closed) in admitted.iter().enumerate().skip(first) {
                        let window = closed - opened;
                        assert!(
                            (last - first + 1) as u32 <= window_bound(window, recovery_ticks),
                            "R {recovery_ticks}, stamp {stamp_step}: {} admissions in {window} host ticks",
                            last - first + 1
                        );
                    }
                }
                assert!(admitted.len() as u32 >= 600 / recovery_ticks);
            }
        }
    }

    /// The bound holds for arbitrary stamps: wrapped jumps, stamps behind the
    /// recorded recovery, and releases stamped anywhere, charged or not.
    #[test]
    fn cadence_arbitrary_stamps_and_releases_cannot_exceed_window_bound() {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for recovery_ticks in [1, 8, 30] {
            let mut cadence = ActivationCadence::default();
            let mut admitted = Vec::new();
            for host_tick in 0..2000u32 {
                for _ in 0..3 {
                    let start_tick = next() as u32;
                    if !cadence.host_time_allows(0, host_tick)
                        || cadence.client_spacing(weapon(), 0, start_tick)
                            == CadenceVerdict::Refused
                    {
                        continue;
                    }
                    let token = token(start_tick);
                    cadence.begin(token, 0, host_tick);
                    let roll = next();
                    if roll & 1 == 1 {
                        cadence.release(token, (roll >> 8) as u32, host_tick);
                    }
                    cadence.recovery_began(
                        token,
                        weapon(),
                        recovery_ticks,
                        roll & 2 == 2,
                        host_tick,
                    );
                    admitted.push(host_tick);
                }
            }
            assert!(admitted.len() as u32 >= 2000 / (recovery_ticks + CADENCE_TOLERANCE_TICKS));
            for (first, &opened) in admitted.iter().enumerate() {
                for (last, &closed) in admitted.iter().enumerate().skip(first) {
                    let window = closed - opened;
                    assert!(
                        (last - first + 1) as u32 <= window_bound(window, recovery_ticks),
                        "R {recovery_ticks}: {} admissions in {window} host ticks",
                        last - first + 1
                    );
                }
            }
        }
    }

    /// A host weapon in this model: the machine's own cooldown, which an
    /// eligible start zeroes and a fired shot restarts, as `guard_initiation`
    /// and the machine do. It counts down every host tick, never slower.
    struct HostWeapon {
        weapon: EntityId,
        slot: u8,
        recovery_ticks: u32,
        cool_at: u32,
        admitted: Vec<u32>,
    }

    impl HostWeapon {
        fn new(raw: u32, slot: u8, recovery_ticks: u32) -> Self {
            Self {
                weapon: EntityId::from_raw(raw),
                slot,
                recovery_ticks,
                cool_at: 0,
                admitted: Vec::new(),
            }
        }

        /// One stamped start through the lane, the guard, and the machine.
        fn attempt(
            &mut self,
            cadence: &mut ActivationCadence,
            start_tick: u32,
            lane: ActivationLane,
            host_tick: u32,
        ) -> Option<CadenceVerdict> {
            if !cadence.host_time_allows(self.slot, host_tick) {
                return None;
            }
            let verdict = cadence.client_spacing(self.weapon, self.slot, start_tick);
            let cooled = verdict == CadenceVerdict::Eligible || host_tick >= self.cool_at;
            if verdict == CadenceVerdict::Refused || !cooled {
                return None;
            }
            let token = ActivationToken { start_tick, lane };
            cadence.begin(token, self.slot, host_tick);
            cadence.recovery_began(token, self.weapon, self.recovery_ticks, false, host_tick);
            self.cool_at = host_tick + self.recovery_ticks;
            self.admitted.push(host_tick);
            Some(verdict)
        }

        /// The host charges a departed weapon's owed ticks to its cooldown.
        fn depart(&mut self, owed: impl IntoIterator<Item = (EntityId, u32)>, host_tick: u32) {
            for (weapon, ticks) in owed {
                if weapon == self.weapon {
                    self.cool_at = self.cool_at.max(host_tick + ticks);
                }
            }
        }

        fn assert_window_bound(&self, label: &str) {
            let recovery = self.recovery_ticks;
            for (first, &opened) in self.admitted.iter().enumerate() {
                for (last, &closed) in self.admitted.iter().enumerate().skip(first) {
                    let window = closed - opened;
                    assert!(
                        (last - first + 1) as u32 <= window_bound(window, recovery),
                        "{label}, R {recovery}: {} admissions in {window} host ticks",
                        last - first + 1
                    );
                }
            }
        }
    }

    fn xorshift(seed: u64) -> impl FnMut() -> u64 {
        let mut state = seed;
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        }
    }

    /// Two weapons in two slots, each start stamped arbitrarily: every weapon
    /// keeps its own record through the switches and its own window bound.
    #[test]
    fn cadence_alternating_weapons_under_stamping_each_keep_their_own_bound() {
        let mut next = xorshift(0x2545_f491_4f6c_dd1d);
        for (recovery_a, recovery_b) in [(8, 12), (1, 30), (9, 9)] {
            let mut cadence = ActivationCadence::default();
            let mut weapons = [
                HostWeapon::new(9, 0, recovery_a),
                HostWeapon::new(10, 1, recovery_b),
            ];
            let mut unrecorded = [0; 2];
            let mut stamp = 1000u32;
            for host_tick in 0..1500u32 {
                for attempt in 0..6u32 {
                    // Alternate weapons every few attempts; stamps lead,
                    // trail, or jump arbitrarily.
                    let index = ((host_tick + attempt) / 3 % 2) as usize;
                    stamp = match next() % 4 {
                        0 => next() as u32,
                        1 => stamp.wrapping_sub(5),
                        _ => stamp.wrapping_add(weapons[index].recovery_ticks),
                    };
                    let lane = if next() & 1 == 0 {
                        ActivationLane::Primary
                    } else {
                        ActivationLane::Secondary
                    };
                    if weapons[index].attempt(&mut cadence, stamp, lane, host_tick)
                        == Some(CadenceVerdict::Unrecorded)
                    {
                        unrecorded[index] += 1;
                    }
                }
            }
            for weapon in &weapons {
                weapon.assert_window_bound("alternating");
                assert!(
                    weapon.admitted.len() as u32
                        >= 1500 / (weapon.recovery_ticks + CADENCE_TOLERANCE_TICKS) / 2,
                    "each weapon still fires: {}",
                    weapon.admitted.len()
                );
            }
            assert_eq!(
                unrecorded,
                [1, 1],
                "a switch never loses a weapon's record; only its first start is unrecorded"
            );
        }
    }

    /// The host player's cooldown freezes while its weapon is holstered. A
    /// stamping client switching A→B→A must not find that time already counted:
    /// its first A start after the switch waits for the frozen remainder, less
    /// only the tolerance every cadence path grants.
    #[test]
    fn cadence_switch_back_under_stamping_cannot_beat_the_host_players_frozen_remainder() {
        const LONG: u32 = 30;
        let mut cadence = ActivationCadence::default();
        let mut rifle = HostWeapon::new(9, 0, LONG);
        let mut stamp = 1000u32;
        assert_eq!(
            rifle.attempt(&mut cadence, stamp, ActivationLane::Primary, 0),
            Some(CadenceVerdict::Unrecorded)
        );
        // Wielded for two host ticks, then holstered while the client fires slot 1.
        let (holstered_at, back_at) = (3u32, 43u32);
        for host_tick in 1..back_at {
            cadence.holster_others(u8::from(host_tick >= holstered_at), host_tick, |_| false);
        }
        // The host player's cooldown counts wielded ticks only.
        let host_player_earliest = LONG + (back_at - holstered_at);
        let mut first = None;
        for host_tick in back_at..back_at + LONG {
            cadence.holster_others(0, host_tick, |_| false);
            stamp = stamp.wrapping_add(1000);
            if first.is_none()
                && rifle
                    .attempt(&mut cadence, stamp, ActivationLane::Primary, host_tick)
                    .is_some()
            {
                first = Some(host_tick);
            }
        }
        assert_eq!(
            first,
            Some(host_player_earliest - CADENCE_TOLERANCE_TICKS),
            "counting the holstered ticks would have admitted it on host tick {back_at}"
        );
    }

    // Regression: a backlog's commands after a weapon switch resolved while the
    // earlier weapon's start still waited in the lane. Naming the new slot, they
    // froze that weapon's recovery under its own start, which then waited until
    // its claim lagged past the allowance and was refused.
    #[test]
    fn cadence_retained_start_is_not_frozen_by_a_switch_stamped_after_it() {
        use super::super::{HostCommandQueues, neutral_sim_command};
        const CLIENT: u64 = 7;
        const LONG: u32 = 12;
        // The client fires slot 1 at ticks 2 and 14, one recovery apart, then
        // switches to slot 0 from tick 15.
        let stamped = |tick: u32| {
            let mut command = crate::netcode::wire_convert::sim_command_to_input(
                &neutral_sim_command(0.0),
                tick,
                0.0,
            );
            command.movement.firing_slot = u8::from(tick <= 14);
            if tick == 2 || tick == 14 {
                command.activation.initiation = Some(postretro_net::wire::WireActivationToken {
                    start_tick: tick,
                    lane: 0,
                });
            }
            command
        };
        let mut queues = HostCommandQueues::new();
        for tick in 0..2 {
            assert!(queues.ingest(CLIENT, &stamped(tick)));
        }
        let mut first = None;
        for tick in 2..5 {
            assert!(queues.ingest(CLIENT, &stamped(tick)));
            let resolved = queues.resolve_tick(CLIENT).unwrap();
            first = first.or(resolved.command.activation.initiation);
        }
        let first = first.expect("the first start is delivered on its own tick");
        queues.activation_recovery_began(CLIENT, first, weapon(), LONG, false);
        // A stall delivers the rest at once; the trim keeps only commands that
        // already name slot 0.
        for tick in 5..=40 {
            assert!(queues.ingest(CLIENT, &stamped(tick)));
        }
        let resolved: Vec<_> = (41..47)
            .map(|tick| {
                queues.ingest(CLIENT, &stamped(tick));
                queues.resolve_tick(CLIENT).unwrap()
            })
            .collect();
        assert!(resolved[0].client_tick > 14, "the backlog takes the trim");
        assert!(resolved.iter().all(|r| r.rejected_activation.is_none()));
        let delivered = resolved
            .iter()
            .position(|r| {
                r.command
                    .activation
                    .initiation
                    .is_some_and(|token| token.start_tick == 14)
            })
            .expect("the slot 1 start is delivered");
        assert_eq!(resolved[delivered].command.firing_slot, 1);
        // Host time since the first start's credit covers the recovery, less the
        // tolerance, on the third host tick after it.
        assert_eq!(
            delivered,
            (LONG - CADENCE_TOLERANCE_TICKS - 1) as usize,
            "held only by host time, never by the later switch"
        );
    }

    #[test]
    fn cadence_release_clears_only_departed_weapons_and_reports_owed_credit() {
        let mut cadence = ActivationCadence::default();
        let mut rifle = HostWeapon::new(9, 0, R);
        let mut launcher = HostWeapon::new(10, 1, 30);
        assert!(
            rifle
                .attempt(&mut cadence, 100, ActivationLane::Primary, 0)
                .is_some()
        );
        // Compressed delivery credits the next start at its client spacing: 8.
        assert!(
            rifle
                .attempt(&mut cadence, 108, ActivationLane::Primary, 1)
                .is_some()
        );
        assert!(
            launcher
                .attempt(&mut cadence, 109, ActivationLane::Primary, 1)
                .is_some()
        );
        let held = |slot: usize| (slot == 1).then(|| EntityId::from_raw(10));
        let owed: Vec<_> = cadence.release_departed(held, 3).collect();
        assert_eq!(
            owed,
            vec![(rifle.weapon, 8 + R - 3)],
            "the dropped rifle owes one recovery after its credit, not after its host fire"
        );
        assert_eq!(
            cadence.client_spacing(rifle.weapon, 0, 1000),
            CadenceVerdict::Unrecorded,
            "a departed weapon's record can never authorize"
        );
        assert!(cadence.host_time_allows(0, 3));
        assert_eq!(
            cadence.client_spacing(launcher.weapon, 1, 139),
            CadenceVerdict::Eligible,
            "the weapon still held keeps its record"
        );
        assert_eq!(cadence.release_departed(held, 3).count(), 0);
        // A weapon handed into a slot whose record it never made displaces it.
        let other = EntityId::from_raw(11);
        cadence.begin(token(140), 1, 40);
        cadence.recovery_began(token(140), other, R, false, 40);
        let held = |slot: usize| (slot == 1).then_some(other);
        assert_eq!(
            cadence.release_departed(held, 40).collect::<Vec<_>>(),
            vec![(launcher.weapon, 0)]
        );
    }

    /// Dropping and picking a weapon back up between stamped starts cannot
    /// restart its credit: its cooldown carries what the record owed.
    #[test]
    fn cadence_drop_and_pickup_under_stamping_cannot_exceed_window_bound() {
        let mut next = xorshift(0x9e37_79b9_7f4a_7c15);
        for recovery_ticks in [1, 4, 8, 9, 30] {
            for stamp_step in [recovery_ticks, 1000] {
                let mut cadence = ActivationCadence::default();
                let mut rifle = HostWeapon::new(9, 0, recovery_ticks);
                let mut start_tick = u32::MAX - 500;
                let mut away_until = 0;
                for host_tick in 0..900u32 {
                    let held = host_tick >= away_until;
                    if held && next().is_multiple_of(5) {
                        // Drop now, pick up one to three ticks later.
                        away_until = host_tick + 1 + (next() % 3) as u32;
                    }
                    let holding = host_tick >= away_until;
                    let owed: Vec<_> = cadence
                        .release_departed(
                            |slot| (holding && slot == 0).then_some(rifle.weapon),
                            host_tick,
                        )
                        .collect();
                    rifle.depart(owed, host_tick);
                    if !holding {
                        continue;
                    }
                    for _ in 0..4 {
                        if rifle
                            .attempt(&mut cadence, start_tick, ActivationLane::Primary, host_tick)
                            .is_some()
                        {
                            start_tick = start_tick.wrapping_add(stamp_step);
                        }
                    }
                }
                rifle.assert_window_bound(&format!("drop and pickup, stamp {stamp_step}"));
                assert!(rifle.admitted.len() as u32 >= 900 / (recovery_ticks + 3) / 3);
            }
        }
    }
}
