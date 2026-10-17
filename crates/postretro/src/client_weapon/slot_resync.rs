// Connected-client safety net for a host active slot that diverged from the
// owner's: detects a persistent mismatch and asks for one re-declaration.
// See: context/lib/networking.md §Combat authority

/// Continuous mismatch, in seconds, before the client re-declares its active
/// slot. A switch the host has accepted shows in the owner-private projection
/// one round trip plus a snapshot interval later; one second clears any
/// playable round trip and snapshot lag, so a mismatch that outlives it is
/// divergence rather than a projection still catching up.
pub(crate) const ACTIVE_SLOT_RESYNC_WINDOW_SECS: f32 = 1.0;
// A 250 ms round trip plus a 100 ms snapshot delay is generous for co-op; the
// window must clear both with margin.
const _: () = assert!(ACTIVE_SLOT_RESYNC_WINDOW_SECS > 0.25 + 0.1);

/// What the detector reads on one client tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActiveSlotResyncObservation {
    /// A switch declaration the host has not answered is in flight.
    pub declarations_pending: bool,
    /// The local inventory has a switch target or origin, or its active
    /// weapon is lowering or raising.
    pub local_switching: bool,
    pub local_active_slot: usize,
    /// The host slot every owner-private weapon sample names
    /// ([`postretro_sim::weapon::ReplicatedWeaponProjection::agreed_slot`]).
    pub projected_slot: Option<usize>,
}

/// Detects a host whose active slot for this client's pawn stays off the
/// client's own selection, and re-declares the client's slot once per window.
///
/// The client owns selection, and the player sees the weapon they believe they
/// hold, so the host converges to the client rather than the reverse. A false
/// positive (a projection still lagging) costs one idempotent accept on the
/// host. Each declaration restarts the window, so a refusal (a host reload
/// that blocks switching) retries after another full window, never in a loop.
#[derive(Debug, Default)]
pub(crate) struct ActiveSlotResync {
    mismatch_secs: f32,
}

impl ActiveSlotResync {
    /// Forget any accumulated mismatch: no pawn, no endpoint, or a new level.
    pub fn reset(&mut self) {
        self.mismatch_secs = 0.0;
    }

    /// Advance by `dt_secs` and return the slot to re-declare, if due.
    pub fn observe(
        &mut self,
        observation: ActiveSlotResyncObservation,
        dt_secs: f32,
    ) -> Option<usize> {
        let mismatched = !observation.declarations_pending
            && !observation.local_switching
            && observation
                .projected_slot
                .is_some_and(|slot| slot != observation.local_active_slot);
        if !mismatched {
            self.reset();
            return None;
        }
        self.mismatch_secs += dt_secs.max(0.0);
        if self.mismatch_secs < ACTIVE_SLOT_RESYNC_WINDOW_SECS {
            return None;
        }
        self.reset();
        Some(observation.local_active_slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: f32 = 1.0 / 60.0;

    fn mismatch() -> ActiveSlotResyncObservation {
        ActiveSlotResyncObservation {
            declarations_pending: false,
            local_switching: false,
            local_active_slot: 0,
            projected_slot: Some(2),
        }
    }

    /// Ticks observed until the first re-declaration, if any within `limit`.
    fn ticks_until_declaration(
        resync: &mut ActiveSlotResync,
        observation: ActiveSlotResyncObservation,
        limit: usize,
    ) -> Option<usize> {
        (1..=limit).find(|_| resync.observe(observation, TICK).is_some())
    }

    fn window_ticks() -> usize {
        (ACTIVE_SLOT_RESYNC_WINDOW_SECS / TICK).ceil() as usize
    }

    #[test]
    fn a_mismatch_shorter_than_the_window_declares_nothing() {
        let mut resync = ActiveSlotResync::default();
        assert_eq!(
            ticks_until_declaration(&mut resync, mismatch(), window_ticks() - 2),
            None
        );
        // Converging just before the window resets it.
        let converged = ActiveSlotResyncObservation {
            projected_slot: Some(0),
            ..mismatch()
        };
        assert_eq!(resync.observe(converged, TICK), None);
        assert_eq!(
            ticks_until_declaration(&mut resync, mismatch(), window_ticks() - 2),
            None
        );
    }

    #[test]
    fn a_mismatch_lasting_the_window_declares_the_local_slot_exactly_once() {
        let mut resync = ActiveSlotResync::default();
        let mut declared = Vec::new();
        // One tick past the window absorbs float accumulation.
        for _ in 0..=window_ticks() {
            declared.extend(resync.observe(mismatch(), TICK));
        }
        assert_eq!(declared, vec![0], "the client's own slot, once");
    }

    #[test]
    fn a_pending_declaration_or_local_switch_resets_the_window() {
        for interrupt in [
            ActiveSlotResyncObservation {
                declarations_pending: true,
                ..mismatch()
            },
            ActiveSlotResyncObservation {
                local_switching: true,
                ..mismatch()
            },
            ActiveSlotResyncObservation {
                projected_slot: None,
                ..mismatch()
            },
        ] {
            let mut resync = ActiveSlotResync::default();
            assert_eq!(
                ticks_until_declaration(&mut resync, mismatch(), window_ticks() - 2),
                None
            );
            assert_eq!(resync.observe(interrupt, TICK), None, "{interrupt:?}");
            assert_eq!(
                ticks_until_declaration(&mut resync, interrupt, 4 * window_ticks()),
                None,
                "{interrupt:?} never declares"
            );
            assert_eq!(
                ticks_until_declaration(&mut resync, mismatch(), window_ticks() - 2),
                None,
                "{interrupt:?} restarted the window"
            );
        }
    }

    #[test]
    fn a_refused_redeclaration_retries_only_after_another_full_window() {
        let mut resync = ActiveSlotResync::default();
        let first = ticks_until_declaration(&mut resync, mismatch(), 4 * window_ticks())
            .expect("a persistent mismatch declares");
        // The refusal settles a round trip later; the host still projects slot 2.
        let in_flight = ActiveSlotResyncObservation {
            declarations_pending: true,
            ..mismatch()
        };
        for _ in 0..15 {
            assert_eq!(resync.observe(in_flight, TICK), None);
        }
        let second = ticks_until_declaration(&mut resync, mismatch(), 4 * window_ticks())
            .expect("the detector retries");
        assert_eq!(second, first, "a full window again, no tight loop");
        // Even with no pending interval between declarations, the window holds.
        let third = ticks_until_declaration(&mut resync, mismatch(), 4 * window_ticks())
            .expect("the detector retries");
        assert_eq!(third, first);
    }
}
