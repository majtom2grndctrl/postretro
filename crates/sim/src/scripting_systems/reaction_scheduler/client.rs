// Client role of the `wait` scheduler: which tails park, and the host level
// tick a `levelLoad`-class tail counts on.
// See: context/lib/scripting.md §12 (Steps after a `wait`)

use postretro_entities::{MachineRole, SequenceTarget, SlotTable, primitive_is_machine_local};
use postretro_scripting_core::data_descriptors::SequenceStep;

use super::{Countdown, DueTick, InstanceKey, ReactionScheduler, SchedulerRole, TickClock};

/// Whether a client has anything to land in `tail`: a machine-local system
/// step, or a `fire` whose target may hold one. Every other step reaches the
/// client by replication.
pub(super) fn tail_has_landing_work(tail: &[SequenceStep], slot_table: &SlotTable) -> bool {
    tail.iter().any(|step| match step.id {
        SequenceTarget::Fire => true,
        SequenceTarget::System => {
            primitive_is_machine_local(&step.primitive, &step.args, slot_table, MachineRole::Client)
        }
        _ => false,
    })
}

impl ReactionScheduler {
    /// Client only, once per frame before the landing drain: read the host's
    /// level tick (`None` until a snapshot and a time-sync sample anchor it)
    /// and expire every host-timeline instance due by it. The host counts the
    /// same fixed ticks it lands its own tails on, so a host stall clamps both
    /// alike. The level's first reading, less this client's own ticks since
    /// reveal ([`Self::level_tick`]), fixes the host tick this client revealed
    /// at; a segment due before that is a late joiner's, which converges
    /// instead of replaying.
    pub fn advance_host_clock(&self, host_level_tick: Option<f64>) {
        let mut state = self.state.borrow_mut();
        if state.role != SchedulerRole::Client {
            return;
        }
        let Some(reading) = host_level_tick.filter(|tick| tick.is_finite()) else {
            return;
        };
        let now = reading.max(0.0).floor() as u64;
        state.host_tick_now = Some(now);
        // Judge lateness by the reveal, never by the reading: the first
        // reading waits for a snapshot and a time-sync sample, so it trails
        // the reveal, and a client live before a beat would otherwise skip it.
        let since_reveal = state.level_tick() as f64;
        state
            .join_host_tick
            .get_or_insert((reading - since_reveal).max(0.0).floor() as u64);
        let current = state.frame_counter;
        let due: Vec<InstanceKey> = state
            .instances
            .iter()
            .filter(|(_, instance)| {
                instance.enrolled_frame != current
                    && matches!(
                        instance.countdown,
                        Countdown::Due(DueTick { clock: TickClock::Host, tick }) if tick <= now
                    )
            })
            .map(|(key, _)| key.clone())
            .collect();
        state.expire(due);
    }

    /// Client only: a host-timeline landing due before the host tick this
    /// client revealed at belongs to a late joiner, which converges state
    /// instead of replaying. Lateness compares host ticks only; an own-clock
    /// landing is never late.
    pub(super) fn is_late(&self, due: DueTick) -> bool {
        let state = self.state.borrow();
        due.clock == TickClock::Host && state.join_host_tick.is_some_and(|join| due.tick < join)
    }
}
