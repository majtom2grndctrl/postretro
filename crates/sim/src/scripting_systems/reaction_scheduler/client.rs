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
    /// every host-timeline instance due by it. The level's first reading fixes
    /// which segments a late joiner converges instead of replaying.
    pub fn advance_host_clock(&self, host_time_secs: Option<f64>) {
        let mut state = self.state.borrow_mut();
        if state.role != SchedulerRole::Client {
            return;
        }
        let Some(host_time) = host_time_secs.filter(|time| time.is_finite()) else {
            return;
        };
        let now = host_tick_at(host_time);
        state.host_tick_now = Some(now);
        state.first_host_reading.get_or_insert(now);
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

    /// Client only: a landing due before the level's first host-clock reading
    /// belongs to a late joiner, which converges state instead of replaying.
    pub(super) fn is_late(&self, due_host_tick: Option<u64>) -> bool {
        let state = self.state.borrow();
        match (due_host_tick, state.first_host_reading) {
            (Some(due), Some(first)) => due < first,
            _ => false,
        }
    }

    /// Client only: expire host-timeline instances a landing just parked that
    /// are already late, so a late joiner's chain of due segments resolves at
    /// its first reading rather than one frame per segment. Returns whether
    /// any expired.
    pub(super) fn expire_late(&self) -> bool {
        let mut state = self.state.borrow_mut();
        let Some(first) = state.first_host_reading else {
            return false;
        };
        let late: Vec<InstanceKey> = state
            .instances
            .iter()
            .filter(|(_, instance)| matches!(instance.countdown, Countdown::HostTick(due) if due < first))
            .map(|(key, _)| key.clone())
            .collect();
        let any = !late.is_empty();
        state.expire(late);
        any
    }

    /// Set the host tick a host-timeline enrollment counts from while one
    /// client landing drains; cleared when the returned guard drops.
    pub(super) fn begin_host_anchor(&self, due_host_tick: Option<u64>) -> HostAnchorGuard<'_> {
        let previous = std::mem::replace(
            &mut self.state.borrow_mut().current_host_anchor,
            due_host_tick,
        );
        HostAnchorGuard {
            scheduler: self,
            previous,
        }
    }
}

pub(super) struct HostAnchorGuard<'a> {
    scheduler: &'a ReactionScheduler,
    previous: Option<u64>,
}

impl Drop for HostAnchorGuard<'_> {
    fn drop(&mut self) {
        self.scheduler.state.borrow_mut().current_host_anchor = self.previous;
    }
}
