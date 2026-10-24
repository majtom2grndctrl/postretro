// State-apply and UI handoff for the four display-only timer streams.
// See: context/lib/networking.md §What gates, and what replicates instead

use super::super::{ClientStateApply, StateApplyOutcome};
use super::NAMES;
use postretro_entities::{SlotTable, SlotValue};

impl ClientStateApply {
    /// Observe one accepted newest snapshot only after its state batch commits.
    /// Omitted and held-id records sample the committed value, while refresh
    /// exclusions cannot masquerade as a held-value sample.
    pub(crate) fn accept_timed_slot_sample(
        &mut self,
        server_tick: u32,
        _slot_table: &SlotTable,
        outcome: &StateApplyOutcome,
    ) {
        if !outcome.state_batch_applied {
            return;
        }
        let mut excluded = [false; 4];
        if let Some(schema) = &self.schema {
            for request in &outcome.refresh_requests {
                if let Some(entry) =
                    schema.entry_for(postretro_net::state_slots::StateSlotId(request.slot_id))
                    && let Some(index) = NAMES.iter().position(|name| *name == entry.name)
                {
                    excluded[index] = true;
                }
            }
        }
        self.timed_slot_targets
            .accept(server_tick, &self.weapon_projection, excluded);
    }

    /// Advance the sample timeline once per client frame. `sample_tick` is the
    /// initialized server estimate minus half the smoothed round-trip time, in
    /// server ticks. It is not a snapshot-arrival anchor.
    pub fn advance_timed_slot_targets(&mut self, sample_tick: Option<f64>, frame_dt_secs: f64) {
        self.timed_slot_targets.advance(sample_tick, frame_dt_secs);
    }

    /// Substitute display targets in the UI's frozen value map. This never writes
    /// the authoritative store or the committed weapon projection.
    pub fn apply_timed_slot_targets(
        &self,
        values: &mut std::collections::HashMap<String, SlotValue>,
    ) {
        self.timed_slot_targets.apply(values);
    }

    pub(crate) fn reset_timed_slot_targets(&mut self) {
        self.timed_slot_targets = super::TimedSlotTargets::default();
    }

    #[cfg(test)]
    pub(crate) fn timed_slot_debug(&self) -> super::TimedSlotDebug {
        self.timed_slot_targets.debug()
    }
}
