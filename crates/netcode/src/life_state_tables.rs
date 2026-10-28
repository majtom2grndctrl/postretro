//! Revision-addressed host life profiles. Control and snapshots can arrive in
//! either order; a state id is meaningful only under its own installed table.
use std::collections::VecDeque;

use postretro_combat_model::TuningPayload;
use postretro_foundation::{
    LifeStateProfile, PlayerMovementComponent, UNDECLARED_LIFE_STATE, carry_life_state_stamps,
    restamp_life_state,
};

#[derive(Debug, Clone)]
pub struct LifeStateTable {
    pub revision: u8,
    pub names: Vec<String>,
    pub profiles: Vec<(u8, LifeStateProfile)>,
}

impl LifeStateTable {
    pub fn profile(&self, id: u8) -> Option<&LifeStateProfile> {
        self.profiles
            .iter()
            .find_map(|(key, profile)| (*key == id).then_some(profile))
    }
}

/// At most four recent revisions plus the live pawn's protected revision.
#[derive(Debug, Clone, Default)]
pub struct LifeStateTables {
    tables: VecDeque<LifeStateTable>,
}

impl LifeStateTables {
    pub fn install(&mut self, payload: &TuningPayload, pinned: Option<u8>) {
        let revision = payload.life_state_table_revision;
        self.tables.retain(|table| table.revision != revision);
        self.tables.push_back(LifeStateTable {
            revision,
            names: payload.life_state_names.clone(),
            profiles: payload.life_state_profiles.clone(),
        });
        // The newest four are always retained. A pin outside that window adds
        // one entry, and ceases to protect it on the next install after adoption.
        let recent_start = self.tables.len().saturating_sub(4);
        let mut index = 0;
        self.tables.retain(|table| {
            let keep = index >= recent_start || Some(table.revision) == pinned;
            index += 1;
            keep
        });
    }

    pub fn table(&self, revision: u8) -> Option<&LifeStateTable> {
        self.tables.iter().find(|table| table.revision == revision)
    }

    pub fn name(&self, pawn: &PlayerMovementComponent) -> &str {
        self.table(pawn.life_state_table_revision)
            .and_then(|table| table.names.get(usize::from(pawn.life_state)))
            .map_or("", String::as_str)
    }

    pub fn clear(&mut self) {
        self.tables.clear();
    }

    /// Tuning rebuilds carry fields and stamps first. A removed same-revision
    /// profile keeps its stamps until the host's remapped state arrives.
    pub(crate) fn restamp_carried(&self, pawn: &mut PlayerMovementComponent) {
        if pawn.life_state == UNDECLARED_LIFE_STATE {
            restamp_life_state(pawn, None);
        } else if let Some(profile) = self
            .table(pawn.life_state_table_revision)
            .and_then(|table| table.profile(pawn.life_state))
        {
            restamp_life_state(pawn, Some(profile));
        }
    }

    /// Run after wire merge and history pruning, before replay. If a snapshot
    /// outran its payload, preserve the last understood state and rebase elapsed
    /// onto the replay baseline instead of counting its tail a second time.
    pub(crate) fn gate_merged(
        &self,
        pawn: &mut PlayerMovementComponent,
        previous: &PlayerMovementComponent,
        replay_ticks: usize,
    ) {
        let table = self.table(pawn.life_state_table_revision);
        let known = table.is_some_and(|table| {
            pawn.life_state == UNDECLARED_LIFE_STATE || table.profile(pawn.life_state).is_some()
        });
        if known {
            self.restamp_carried(pawn);
        } else {
            pawn.life_state = previous.life_state;
            pawn.life_state_table_revision = previous.life_state_table_revision;
            pawn.life_state_elapsed_ticks = previous
                .life_state_elapsed_ticks
                .saturating_sub(u32::try_from(replay_ticks).unwrap_or(u32::MAX));
            carry_life_state_stamps(previous, pawn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_combat_model::{EnvironmentTuning, FluidMovementTable};
    use postretro_foundation::enter_life_state;

    fn payload(revision: u8, profiles: Vec<(u8, LifeStateProfile)>) -> TuningPayload {
        let mut payload = TuningPayload::new(
            None,
            std::array::from_fn(|_| None),
            EnvironmentTuning {
                level_gravity: -9.81,
                fluids: FluidMovementTable::default(),
            },
        );
        payload.life_state_names = vec!["alive".into(), "down".into(), "x".into()];
        payload.life_state_table_revision = revision;
        payload.life_state_profiles = profiles;
        payload
    }

    fn pawn(
        id: u8,
        revision: u8,
        elapsed: u32,
        profile: &LifeStateProfile,
    ) -> PlayerMovementComponent {
        let mut pawn = crate::predict_reconcile_harness_test_fixtures::component();
        enter_life_state(&mut pawn, id, profile, revision);
        pawn.life_state_elapsed_ticks = elapsed;
        pawn
    }

    #[test]
    fn o26_snapshot_before_control_keeps_profile_and_rebases_replay_elapsed() {
        let mut blocked = LifeStateProfile::default();
        blocked.fire = false;
        blocked.movement.speed_scale = 0.3;
        let previous = pawn(1, 7, 40, &blocked);
        let mut tables = LifeStateTables::default();
        tables.install(&payload(7, vec![(1, blocked.clone())]), Some(7));
        let mut merged = pawn(0, 8, 2, &LifeStateProfile::default());
        tables.gate_merged(&mut merged, &previous, 6);
        assert_eq!(
            (
                merged.life_state,
                merged.life_state_table_revision,
                merged.life_state_elapsed_ticks
            ),
            (1, 7, 34)
        );
        assert!(!merged.life_fire);
        assert_eq!(merged.life_speed_scale, 0.3);
        // Re-running the six commands leaves elapsed at the original predicted
        // frontier. The new table then permits adoption of the shifted id.
        merged.life_state_elapsed_ticks += 6;
        assert_eq!(merged.life_state_elapsed_ticks, 40);
        tables.install(&payload(8, vec![(0, LifeStateProfile::default())]), Some(7));
        let mut next = pawn(0, 8, 3, &blocked);
        tables.gate_merged(&mut next, &merged, 6);
        assert_eq!(
            (
                next.life_state,
                next.life_state_table_revision,
                next.life_state_elapsed_ticks
            ),
            (0, 8, 3)
        );
        assert!(next.life_fire);
        let mut floored = pawn(0, 9, 9, &LifeStateProfile::default());
        tables.gate_merged(&mut floored, &previous, 100);
        assert_eq!(floored.life_state_elapsed_ticks, 0);
    }

    #[test]
    fn o36_two_payloads_before_merge_resolve_each_pawns_own_revision() {
        let mut blocked = LifeStateProfile::default();
        blocked.fire = false;
        let mut tables = LifeStateTables::default();
        tables.install(&payload(10, vec![(1, blocked.clone())]), Some(10));
        tables.install(
            &payload(11, vec![(0, LifeStateProfile::default())]),
            Some(10),
        );
        tables.install(&payload(12, vec![(2, blocked.clone())]), Some(10));
        let live = pawn(1, 10, 50, &blocked);
        let mut rebuilt = crate::predict_reconcile_harness_test_fixtures::component();
        crate::movement_state::carry_live_tick_state(&mut rebuilt, &live);
        rebuilt.life_fire = true;
        tables.restamp_carried(&mut rebuilt);
        assert!(!rebuilt.life_fire);
        assert_eq!(rebuilt.life_state_elapsed_ticks, 50);
        let mut first = pawn(0, 11, 5, &blocked);
        tables.gate_merged(&mut first, &live, 0);
        assert!(first.life_fire);
        let mut second = pawn(2, 12, 6, &LifeStateProfile::default());
        tables.gate_merged(&mut second, &first, 0);
        assert!(!second.life_fire);
        assert_eq!(second.life_state_table_revision, 12);
    }

    #[test]
    fn o42_same_revision_profile_removal_keeps_stamps_until_host_remap() {
        let mut blocked = LifeStateProfile::default();
        blocked.fire = false;
        let mut tables = LifeStateTables::default();
        tables.install(
            &payload(
                4,
                vec![(0, LifeStateProfile::default()), (2, blocked.clone())],
            ),
            Some(4),
        );
        let previous = pawn(2, 4, 60, &blocked);
        // Another archetype keeps x in names, but this pawn no longer has x.
        tables.install(&payload(4, vec![(0, LifeStateProfile::default())]), Some(4));
        let mut rebuilt = crate::predict_reconcile_harness_test_fixtures::component();
        crate::movement_state::carry_live_tick_state(&mut rebuilt, &previous);
        tables.restamp_carried(&mut rebuilt);
        assert!(!rebuilt.life_fire);
        assert_eq!(rebuilt.life_state_elapsed_ticks, 60);
        let mut stale = pawn(2, 4, 55, &LifeStateProfile::default());
        tables.gate_merged(&mut stale, &previous, 5);
        assert_eq!(stale.life_state_elapsed_ticks, 55);
        assert!(!stale.life_fire);
        let mut remapped = pawn(0, 4, 0, &blocked);
        tables.gate_merged(&mut remapped, &previous, 5);
        assert!(remapped.life_fire);
        assert_eq!(remapped.life_state, 0);
    }

    #[test]
    fn history_keeps_four_recent_plus_pin_and_replaces_same_revision() {
        let mut tables = LifeStateTables::default();
        for revision in 0..10 {
            tables.install(&payload(revision, vec![]), Some(0));
        }
        assert_eq!(
            tables
                .tables
                .iter()
                .map(|table| table.revision)
                .collect::<Vec<_>>(),
            [0, 6, 7, 8, 9]
        );
        tables.install(&payload(9, vec![(0, LifeStateProfile::default())]), Some(0));
        assert_eq!(tables.tables.len(), 5);
        assert!(tables.table(9).unwrap().profile(0).is_some());
        tables.install(&payload(10, vec![]), Some(9));
        assert_eq!(
            tables
                .tables
                .iter()
                .map(|table| table.revision)
                .collect::<Vec<_>>(),
            [7, 8, 9, 10]
        );
    }
}
