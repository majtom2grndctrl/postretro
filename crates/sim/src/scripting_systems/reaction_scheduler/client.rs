// Client role of the `wait` scheduler: which tails park, and the host clock a
// `levelLoad`-class tail counts on.
// See: context/lib/scripting.md §12 (Steps after a `wait`)

use postretro_entities::{MachineRole, SequenceTarget, SlotTable, primitive_is_machine_local};
use postretro_scripting_core::data_descriptors::SequenceStep;

use super::{Countdown, InstanceKey, ReactionScheduler, SchedulerRole};

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

/// The host tick a host `script_time` falls in. Host ticks count from the
/// level's host `script_time` 0, where `levelLoad` fires.
fn host_tick_at(host_time_secs: f64) -> u64 {
    let micros = crate::frame_timing::TICK_DURATION.as_micros() as f64;
    (host_time_secs.max(0.0) * 1_000_000.0 / micros).floor() as u64
}

impl ReactionScheduler {
    /// Client only, once per frame before the landing drain: read the host
    /// clock (`None` until a snapshot and an RTT sample anchor it) and expire
    /// every host-timeline instance due by it. `local_time_secs` is this
    /// client's level clock, which counts from its own reveal. The level's
    /// first reading fixes the host tick this client revealed at; a segment
    /// due before that is a late joiner's, which converges instead of
    /// replaying.
    pub fn advance_host_clock(&self, host_time_secs: Option<f64>, local_time_secs: f64) {
        let mut state = self.state.borrow_mut();
        if state.role != SchedulerRole::Client {
            return;
        }
        let Some(host_time) = host_time_secs.filter(|time| time.is_finite()) else {
            return;
        };
        let now = host_tick_at(host_time);
        state.host_tick_now = Some(now);
        // Judge lateness by the reveal, never by the reading: the first
        // reading waits for a snapshot and an RTT sample, so it trails the
        // reveal by half an RTT and a snapshot interval, and a client live
        // before a beat would otherwise skip it.
        let local_time = if local_time_secs.is_finite() {
            local_time_secs.max(0.0)
        } else {
            0.0
        };
        state
            .join_host_tick
            .get_or_insert(host_tick_at(host_time - local_time));
        let current = state.frame_counter;
        let due: Vec<InstanceKey> = state
            .instances
            .iter()
            .filter(|(_, instance)| {
                instance.enrolled_frame != current
                    && matches!(instance.countdown, Countdown::HostTick(due) if due <= now)
            })
            .map(|(key, _)| key.clone())
            .collect();
        state.expire(due);
    }

    /// Client only: a landing due before the host tick this client revealed
    /// at belongs to a late joiner, which converges state instead of
    /// replaying.
    pub(super) fn is_late(&self, due_tick: Option<u64>) -> bool {
        let state = self.state.borrow();
        match (due_tick, state.join_host_tick) {
            (Some(due), Some(join)) => due < join,
            _ => false,
        }
    }

    /// Client only, after a landing batch: expire the host-timeline instances
    /// parked during this drain (`id >= parked_from`) that the latest reading
    /// has already passed. They join the queue beside the segments still due,
    /// so one reading lands a whole due chain in due order, as the host did.
    pub(super) fn catch_up_host_timeline(&self, parked_from: u64) {
        let mut state = self.state.borrow_mut();
        let Some(now) = state.host_tick_now else {
            return;
        };
        let due: Vec<InstanceKey> = state
            .instances
            .iter()
            .filter(|(_, instance)| {
                instance.id >= parked_from
                    && matches!(instance.countdown, Countdown::HostTick(due) if due <= now)
            })
            .map(|(key, _)| key.clone())
            .collect();
        state.expire(due);
    }
}
