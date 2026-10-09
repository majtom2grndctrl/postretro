// Client-domain recovery admission for remote activation starts.
// See: context/lib/networking.md §Combat authority · §Host input command queue
//
// Two tick domains meet here. Client ticks are the client's own fixed-tick
// stamps: start ticks, release ticks, and the client tick at which a recovery
// began. Host ticks are this client's playout clock, `ClientCommandState::host_tick`,
// which advances once per host fixed tick the client is resolved.
//
// The rule: a start is eligible once its client tick is at least the recovery
// after the client tick at which the previous execution's recovery began, and
// host time since that recovery began, plus `CADENCE_TOLERANCE_TICKS`, also
// covers the recovery. The client half is checked when the start binds a
// weapon; the host half holds the start in its retained lane until it passes.
//
// Host time "since that recovery began" is measured from a credited host tick,
// not the tick the host happened to fire. Each execution is credited at the
// client's claimed time, clamped to [host now, host now + tolerance]. Credit
// therefore never trails host time at admission, and an early start carries its
// lead into the next one instead of earning a fresh tolerance each time. A
// charged execution fires at its release, so its credit moves to the release,
// forward only; any other execution ignores releases, as the machine does.
//
// Bound (pinned by the two `cannot_exceed_window_bound` tests): each
// admitted execution's credit is at least one recovery after the last one, the
// first credit in a window is at or after the window opens, and every credit is
// at most `CADENCE_TOLERANCE_TICKS` past its admission. So over any span of W
// host ticks, executions admitted ≤ ⌊(W + CADENCE_TOLERANCE_TICKS) / R⌋ + 1,
// whatever client ticks the client stamps. The constant above W / R is
// 1 + 9 / R: under 2 for any recovery of 150 ms or more, 2.125 at 130 ms.

use crate::prediction::client_tick_le;
use postretro_entities::EntityId;
use postretro_foundation::ActivationToken;

/// 150 ms at 60 Hz: the charge rule's tolerance, applied to cadence.
pub(crate) const CADENCE_TOLERANCE_TICKS: u32 =
    postretro_combat_model::activation::CHARGE_TOLERANCE_TICKS;

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
    firing_slot: u8,
    /// Client tick at which the recovery began.
    client_tick: u32,
    /// Host tick credited to that same moment.
    credit_tick: u32,
    /// `ceil(recovery_ms / tick_ms)`, the client's predicted countdown in ticks.
    recovery_ticks: u32,
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

#[derive(Debug, Default)]
pub(crate) struct ActivationCadence {
    recovery: Option<Recovery>,
    execution: Option<Execution>,
}

impl ActivationCadence {
    /// Host half of the rule for a start that would fire from `firing_slot`.
    pub fn host_time_allows(&self, firing_slot: u8, host_tick: u32) -> bool {
        self.recovery
            .filter(|recovery| recovery.firing_slot == firing_slot)
            .is_none_or(|recovery| {
                host_ticks_since(host_tick, recovery.credit_tick)
                    + i64::from(CADENCE_TOLERANCE_TICKS)
                    >= i64::from(recovery.recovery_ticks)
            })
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
            .recovery
            .filter(|recovery| recovery.weapon == weapon && recovery.firing_slot == firing_slot)
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
        let credit_tick = match self
            .recovery
            .filter(|recovery| recovery.firing_slot == firing_slot)
        {
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
        let offset = host_tick.wrapping_sub(execution.host_tick);
        self.recovery = Some(Recovery {
            weapon,
            firing_slot: execution.firing_slot,
            client_tick: execution.client_tick.wrapping_add(offset),
            credit_tick: execution.credit_tick.wrapping_add(offset),
            recovery_ticks,
        });
    }
}

/// Signed host ticks from `earlier` to `now`; credit may lead host time.
fn host_ticks_since(now: u32, earlier: u32) -> i64 {
    i64::from(now.wrapping_sub(earlier) as i32)
}

/// The client's claimed host tick, clamped to [host now, host now + tolerance].
fn credit(previous_credit: u32, client_span: i32, host_tick: u32) -> u32 {
    let claimed = previous_credit.wrapping_add_signed(client_span);
    let lead = host_ticks_since(claimed, host_tick);
    host_tick.wrapping_add(lead.clamp(0, i64::from(CADENCE_TOLERANCE_TICKS)) as u32)
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
    fn cadence_late_start_never_credits_before_host_admission() {
        let mut cadence = ActivationCadence::default();
        assert!(admit(&mut cadence, 100, 0));
        // Client spacing claims host tick 8, but the start reached the host at 40.
        assert!(admit(&mut cadence, 108, 40));
        assert!(!cadence.host_time_allows(0, 38));
        assert!(cadence.host_time_allows(0, 39));
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

    /// Admissions per host window never exceed ⌊(W + tolerance) / R⌋ + 1,
    /// whatever client ticks the client stamps.
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
                        let bound = (window + CADENCE_TOLERANCE_TICKS) / recovery_ticks + 1;
                        assert!(
                            (last - first + 1) as u32 <= bound,
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
                    let bound = (window + CADENCE_TOLERANCE_TICKS) / recovery_ticks + 1;
                    assert!(
                        (last - first + 1) as u32 <= bound,
                        "R {recovery_ticks}: {} admissions in {window} host ticks",
                        last - first + 1
                    );
                }
            }
        }
    }
}
