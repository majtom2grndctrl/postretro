// CPU application-test access to accepted state batches and timer sample intake.
// See: context/lib/testing_guide.md §3

use super::{ClientStateApply, ReplicatedSlotIdentity};
use postretro_entities::SlotTable;

impl ClientStateApply {
    /// Drive already-accepted host records through the real state-apply/intake
    /// seam for CPU tests in the application crate, without a socket or GPU.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn apply_timed_slot_snapshot_for_test(
        &mut self,
        table: &mut SlotTable,
        identity: &ReplicatedSlotIdentity<'_>,
        sequence: u32,
        tick: u32,
        values: &[(&str, postretro_net::state_slots::WireSlotValue)],
    ) {
        let schema = super::ReplicatedSlotSchema::build(table, identity);
        let records: Vec<_> = values
            .iter()
            .map(
                |(name, value)| postretro_net::state_slots::RawStateSlotRecord {
                    slot_id: schema
                        .entries()
                        .iter()
                        .find(|entry| entry.name == *name)
                        .expect("test names a replicated slot")
                        .slot_id
                        .0,
                    kind: postretro_net::state_slots::STATE_RECORD_KIND_FULL_BASELINE,
                    has_baseline_ref: false,
                    baseline_ref: 0,
                    baseline_id: sequence.wrapping_add(1),
                    value: value.clone(),
                },
            )
            .collect();
        let outcome =
            self.apply_snapshot_state(table, identity, sequence, schema.fingerprint(), &records);
        assert!(outcome.state_batch_applied, "test snapshot must commit");
        self.accept_timed_slot_sample(tick, table, &outcome);
    }
}
