// Bridges between the save files' ledger documents and the in-memory store document.
// See: context/lib/scripting.md §5 (Durable State Store)

use super::*;
use crate::scripting::save_files::{LedgerDocument, SeatRecord};

impl PersistedState {
    /// The store document a world file, a run file and one character compose
    /// into: world and run globals together, the character's per-owner rows
    /// keyed by its claim, and the run's faction sentiment. Overlaying it is
    /// how a selection, Continue or `newRun` reloads the store.
    pub fn from_ledger(
        world: Option<&LedgerDocument>,
        run: Option<&LedgerDocument>,
        character: Option<&SeatRecord>,
    ) -> Self {
        let mut slots = BTreeMap::new();
        for document in [world, run].into_iter().flatten() {
            slots.extend(document.slots.clone());
        }
        let per_owner = character
            .map(|record| {
                record
                    .per_owner
                    .iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            BTreeMap::from([(record.claim.clone(), value.clone())]),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            version: CURRENT_STATE_VERSION,
            slots,
            per_owner,
            faction_sentiment: run
                .map(|document| document.faction_sentiment.clone())
                .unwrap_or_default(),
        }
    }

    /// Global slot values by durable key.
    pub fn slots(&self) -> &BTreeMap<String, PersistedValue> {
        &self.slots
    }

    /// Faction sentiment in its durable form.
    pub fn faction_sentiment(&self) -> &BTreeMap<String, BTreeMap<String, f32>> {
        &self.faction_sentiment
    }

    /// One player's per-owner values by durable key.
    pub fn per_owner_for(&self, claim: &str) -> BTreeMap<String, PersistedValue> {
        self.per_owner
            .iter()
            .filter_map(|(key, rows)| rows.get(claim).map(|value| (key.clone(), value.clone())))
            .collect()
    }
}

/// The durable keys of the committed slots declared `persist: "run"`.
pub fn run_scoped_durable_keys(
    identity: Option<&StoreIdentityLedger>,
    run_scoped_slots: &BTreeSet<String>,
) -> BTreeSet<String> {
    run_scoped_slots
        .iter()
        .filter_map(|name| identity.and_then(|ledger| ledger.durable_key(name)))
        .map(str::to_owned)
        .collect()
}

/// Split global slot values into the world file's and the run file's by
/// durable key.
pub fn split_world_and_run(
    slots: &BTreeMap<String, PersistedValue>,
    run_keys: &BTreeSet<String>,
) -> (
    BTreeMap<String, PersistedValue>,
    BTreeMap<String, PersistedValue>,
) {
    slots
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .partition(|(key, _)| !run_keys.contains(key))
}

/// Return every mod-owned slot to its declared default before a reload
/// overlays saved values: globals take their default, and `seat`'s per-owner
/// values are dropped so they read the default. Engine-owned slots and other
/// seats' per-owner values are untouched, so admitted guests keep theirs.
pub fn reset_mod_slots_for_reload(table: &mut SlotTable, seat: Seat) {
    for (_, record) in table.iter_mut() {
        if record.schema.ownership != SlotOwnership::Mod {
            continue;
        }
        if record.schema.per_owner {
            record.clear_per_seat_value(seat);
        } else {
            let default = record.schema.default.clone();
            record.write_value(default);
        }
    }
}

/// Copy `seat`'s per-owner value into each mod per-owner slot's local value,
/// which UI binds read, so a reload shows the selected character's values.
pub fn project_seat_per_owner_values(table: &mut SlotTable, seat: Seat) {
    for (_, record) in table.iter_mut() {
        if record.schema.ownership == SlotOwnership::Mod && record.schema.per_owner {
            let value = record.per_seat_value(seat).cloned();
            record.write_value(value);
        }
    }
}
