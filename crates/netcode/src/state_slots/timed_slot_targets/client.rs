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
        slot_table: &SlotTable,
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
        self.timed_slot_observations
            .push(std::array::from_fn(|index| {
                (!excluded[index]).then(|| {
                    match slot_table
                        .get(NAMES[index])
                        .and_then(|record| record.value.as_ref())
                    {
                        Some(SlotValue::Number(value)) => Some(*value),
                        _ => None,
                    }
                })
            }));
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

    /// Detect client crossings after local game logic settles, behind the frame's
    /// snapshot-apply witness. Replay only timer thresholds at accepted sample
    /// cadence; all predicates and other thresholds retain frame-final cadence.
    pub fn detect_state_crossings(
        &mut self,
        detector: &mut postretro_scripting_core::state_crossings::CrossingDetector,
        slot_table: &SlotTable,
    ) -> Vec<postretro_scripting_core::state_crossings::CrossingFire> {
        let mut fires = Vec::new();
        for observation in self.timed_slot_observations.drain(..) {
            let mut samples = [("", None); 4];
            let mut count = 0;
            for (name, value) in NAMES.into_iter().zip(observation) {
                if let Some(value) = value {
                    samples[count] = (name, value);
                    count += 1;
                }
            }
            fires.extend(detector.observe_threshold_samples(&samples[..count]));
        }
        fires.extend(detector.detect_excluding_threshold_slots(slot_table, &NAMES));
        fires
    }

    pub(crate) fn reset_timed_slot_targets(&mut self) {
        self.timed_slot_targets = super::TimedSlotTargets::default();
        self.timed_slot_observations.clear();
    }

    #[cfg(test)]
    pub(crate) fn timed_slot_pending_observations(&self) -> &[[Option<Option<f32>>; 4]] {
        &self.timed_slot_observations
    }

    #[cfg(test)]
    pub(crate) fn timed_slot_debug(&self) -> super::TimedSlotDebug {
        self.timed_slot_targets.debug()
    }
}
