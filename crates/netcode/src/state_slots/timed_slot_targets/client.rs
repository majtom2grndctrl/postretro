// State-apply and UI handoff for the four display-only timer streams and the
// received-assist stream, and the helper's committed assist-progress sample.
// See: context/lib/networking.md §What gates, and what replicates instead

use super::super::{ClientStateApply, StateApplyOutcome};
use super::NAMES;
use super::received::RECEIVED_NAME;
use postretro_entities::{SlotTable, SlotValue};

const ASSIST_PROGRESS_NAME: &str = "player.assistProgress";

impl ClientStateApply {
    /// Observe one accepted newest snapshot only after its state batch commits.
    /// Omitted and held-id records sample the committed value, while refresh
    /// exclusions cannot masquerade as a held-value sample.
    /// `received_required_ticks` is the local pawn's
    /// `ClientReplication::received_required_ticks()` after the same snapshot,
    /// so the received stream's slope belongs to the helper whose progress the
    /// sample reports.
    pub(crate) fn accept_timed_slot_sample(
        &mut self,
        server_tick: u32,
        slot_table: &SlotTable,
        outcome: &StateApplyOutcome,
        received_required_ticks: u32,
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
        let received = self.committed_number_sample(slot_table, outcome, RECEIVED_NAME);
        self.timed_slot_targets
            .accept_received(server_tick, received, received_required_ticks);
    }

    /// The committed `player.assistProgress` after this snapshot's state
    /// batch, under the guards the timer samples use: none for a batch that
    /// did not apply or a slot held for a baseline refresh. A batch with no
    /// record for the slot samples the value the client already holds.
    pub(crate) fn assist_progress_sample(
        &self,
        slot_table: &SlotTable,
        outcome: &StateApplyOutcome,
    ) -> Option<f32> {
        self.committed_number_sample(slot_table, outcome, ASSIST_PROGRESS_NAME)
    }

    fn committed_number_sample(
        &self,
        slot_table: &SlotTable,
        outcome: &StateApplyOutcome,
        name: &str,
    ) -> Option<f32> {
        if !outcome.state_batch_applied {
            return None;
        }
        let held_for_refresh = self.schema.as_ref().is_some_and(|schema| {
            outcome.refresh_requests.iter().any(|request| {
                schema
                    .entry_for(postretro_net::state_slots::StateSlotId(request.slot_id))
                    .is_some_and(|entry| entry.name == name)
            })
        });
        if held_for_refresh {
            return None;
        }
        match slot_table.get(name)?.value.as_ref()? {
            SlotValue::Number(value) => Some(*value),
            _ => None,
        }
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

    #[cfg(test)]
    pub(crate) fn received_assist_displayed(&self) -> Option<f64> {
        self.timed_slot_targets.received_displayed()
    }
}
