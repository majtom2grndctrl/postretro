// Join seeds and final characters: per-owner values crossing Control.
// See: context/lib/scripting.md §5 (Durable State Store)

use super::*;

/// Extract one local player's retained per-owner values for the Control-channel
/// join seed. JSON-only unsupported values cannot cross bitcode, so preserve
/// them on disk but omit them from the wire payload with an actionable warning.
pub fn join_seed_from_persisted_state(
    persisted: Option<&PersistedState>,
    local_player_id: Option<[u8; 16]>,
) -> BTreeMap<String, JoinSeedValue> {
    let Some(local_player_id) = local_player_id else {
        return BTreeMap::new();
    };
    let local_player_id = encode_player_claim_id(&PlayerClaimId(local_player_id));
    let mut slots = BTreeMap::new();
    let Some(persisted) = persisted else {
        return slots;
    };
    if !persisted_state_version_is_supported(persisted) {
        return slots;
    }

    for (durable_key, player_values) in &persisted.per_owner {
        let Some(value) = player_values.get(&local_player_id) else {
            continue;
        };
        match JoinSeedValue::try_from(value.clone()) {
            Ok(value) => {
                slots.insert(durable_key.clone(), value);
            }
            Err(()) => log::warn!(
                "[State] join seed skips unsupported persisted value for durable key `{durable_key}`"
            ),
        }
    }
    slots
}

/// The final character a host returns to a closing client: the seat's
/// persisted per-owner values the host replicates to its owner, keyed by
/// durable key, the join seed's mirror. A slot that never replicates is the
/// client's own, and the host's copy is only what its join seed carried, so
/// it is left out. Values a bitcode payload cannot carry, and slots without
/// durable identity, are omitted with a warning.
pub fn final_character_for_seat(
    table: &SlotTable,
    identity: Option<&StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
    seat: Seat,
) -> (BTreeMap<String, JoinSeedValue>, Vec<String>) {
    let mut slots = BTreeMap::new();
    let mut warnings = Vec::new();
    for (name, record) in table.iter() {
        if !is_persisted_per_owner_slot(record)
            || record.schema.network == ReplicationScope::None
            || !committed_store_slots.contains(name)
        {
            continue;
        }
        let Some(value) = record.per_seat_value(seat) else {
            continue;
        };
        let Some(durable_key) = identity.and_then(|ledger| ledger.durable_key(name)) else {
            warnings.push(format!(
                "persistent per-owner state slot `{name}` has no durable identity; omitting it from the final character"
            ));
            continue;
        };
        match value_for_save(name, record, value).map(JoinSeedValue::try_from) {
            Ok(Ok(value)) => {
                slots.insert(durable_key.to_owned(), value);
            }
            Ok(Err(())) => warnings.push(format!(
                "final character omits unsupported value of `{name}`"
            )),
            Err(warning) => warnings.push(warning),
        }
    }
    (slots, warnings)
}

/// What [`apply_join_seed`] did: how many seed entries it wrote, and a
/// warning for each one it rejected.
#[derive(Debug, Default)]
pub struct JoinSeedOutcome {
    pub applied: usize,
    pub warnings: Vec<String>,
}

/// Validate and apply an admitted player's join seed through the same durable
/// identity and schema checks used by the restore path. A join seed is stricter
/// about bounded numbers: out-of-range values are rejected rather than clamped
/// so a client cannot turn a stale or malicious value into a host-side write.
pub fn apply_join_seed(
    table: &mut SlotTable,
    identity: Option<&StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
    seat: Seat,
    slots: BTreeMap<String, JoinSeedValue>,
) -> JoinSeedOutcome {
    let authored_by_key = identity
        .map(|ledger| {
            ledger
                .slots
                .iter()
                .filter(|(name, _)| committed_store_slots.contains(name.as_str()))
                .map(|(name, key)| (key.as_str(), name.as_str()))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut warnings = Vec::new();
    let mut applied = 0;

    for (durable_key, seed_value) in slots {
        let Some(name) = authored_by_key.get(durable_key.as_str()).copied() else {
            warnings.push(format!(
                "join seed contains unknown durable key `{durable_key}`; ignoring it"
            ));
            continue;
        };
        let Some(record) = table.get_mut(name) else {
            warnings.push(format!(
                "join seed durable key `{durable_key}` targets unavailable slot `{name}`; ignoring it"
            ));
            continue;
        };
        if !is_persisted_per_owner_slot(record) {
            warnings.push(format!(
                "join seed targets non-persistent, global, readonly, or engine-owned slot `{name}`; ignoring it"
            ));
            continue;
        }

        let persisted_value = PersistedValue::from(seed_value);
        match restored_value(name, record, &persisted_value) {
            Ok((_value, Some(warning))) => warnings.push(format!(
                "join seed value for slot `{name}` is out of range; ignoring it ({warning})"
            )),
            Ok((value, None)) => {
                record.set_per_seat_value(seat, value);
                applied += 1;
            }
            Err(warning) => warnings.push(format!("join seed {warning}")),
        }
    }

    JoinSeedOutcome { applied, warnings }
}
