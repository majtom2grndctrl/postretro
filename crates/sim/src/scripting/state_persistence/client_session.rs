// A connected client's own saves and its store restore when a session ends.
// See: context/lib/scripting.md §5 (Durable State Store)

use super::*;

/// Build a connected client's save document. It replaces this player's
/// freshly collected values and keeps everything else the retained boot
/// document holds: other players' entries and the client's own world, its
/// globals and faction sentiment, unchanged. The host's replicated world
/// never reaches it.
pub fn collected_per_owner_only_state(
    retained: Option<&PersistedState>,
    collected: BTreeMap<String, BTreeMap<String, PersistedValue>>,
) -> PersistedState {
    let mut per_owner = retained
        .map(|state| state.per_owner.clone())
        .unwrap_or_default();
    for (durable_key, player_values) in collected {
        per_owner
            .entry(durable_key)
            .or_default()
            .extend(player_values);
    }

    PersistedState {
        version: CURRENT_STATE_VERSION,
        // The world it booted with, never the host's projection in its store.
        slots: retained
            .map(|state| state.slots.clone())
            .unwrap_or_default(),
        per_owner,
        faction_sentiment: retained
            .map(|state| state.faction_sentiment.clone())
            .unwrap_or_default(),
    }
}

/// Record a successful client-private save in the retained boot document while
/// preserving any global section that document carried.
pub fn retain_saved_per_owner_state(retained: &mut Option<PersistedState>, saved: PersistedState) {
    if let Some(retained) = retained.as_mut() {
        retained.per_owner = saved.per_owner;
    } else {
        *retained = Some(saved);
    }
}

/// Mirror the client-visible scalar projection of each per-owner slot into the
/// local seat cache. Replication intentionally exposes only this client's
/// scalar value; the cache lets persistence use the same seat-addressed read
/// path as the authoritative host without ever inventing another player's data.
pub fn sync_client_per_owner_projection(table: &mut SlotTable, local_seat: Seat) {
    for (_, record) in table.iter_mut() {
        if !record.schema.per_owner || record.schema.network != ReplicationScope::OwnerPrivatePlayer
        {
            continue;
        }
        let Some(value) = record.value.clone() else {
            continue;
        };
        if record.per_seat_value(local_seat) != Some(&value) {
            record.set_per_seat_value(local_seat, value);
        }
    }
}

/// Restore device-local per-owner values after a connected client learns its
/// host-assigned seat. Owner-private slots arrive through host replication;
/// only local-only slots need this second, seat-aware overlay.
pub fn overlay_client_local_per_owner_state(
    table: &mut SlotTable,
    persisted: &PersistedState,
    identity: Option<&StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
    local_player_id: Option<[u8; 16]>,
    local_seat: Seat,
) -> Vec<String> {
    if !persisted_state_version_is_supported(persisted) {
        return vec![unsupported_state_version_warning(persisted.version)];
    }

    let authored_by_key = authored_names_by_durable_key(identity, committed_store_slots);
    overlay_per_owner_state(
        table,
        persisted,
        &authored_by_key,
        local_player_id,
        local_seat,
        |_, record| record.schema.network == ReplicationScope::None,
    )
}

/// Restore a client's store when its session ends, as single-player on
/// `Seat(0)`. Every slot the host replicated returns to what a fresh boot
/// from `boot` would hold: its declared default, then `boot`'s persisted
/// global value. Per-owner values move from `session_seat` to `Seat(0)`: a
/// slot that never replicates keeps the client's own write, and a replicated
/// persisted one keeps the latest character there when `character_on_seat`.
/// A replicated character the client never received (it never participated),
/// and a local-only value it never held on a session seat (no roster seated
/// it), come from `boot`'s rows for `local_player_id`. Replicated non-persisted per-owner
/// slots return to their defaults; non-replicated globals keep the client's
/// writes. Faction sentiment is the caller's.
pub fn restore_store_after_session(
    table: &mut SlotTable,
    boot: Option<&PersistedState>,
    identity: Option<&StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
    session_seat: Option<Seat>,
    character_on_seat: bool,
    local_player_id: Option<[u8; 16]>,
) -> Vec<String> {
    const SINGLE_PLAYER_SEAT: Seat = Seat(0);
    // Per-owner slots whose single-player value comes from `boot`'s rows: a
    // replicated character this client never received, and a local-only
    // value it never held on a session seat (no roster ever seated it).
    let mut from_boot = BTreeSet::new();
    for (name, record) in table.iter_mut() {
        let replicated = record.schema.network != ReplicationScope::None;
        if record.schema.per_owner {
            let own = session_seat.and_then(|seat| {
                let value = record.written_per_seat_value(seat).cloned();
                record.clear_per_seat_value(seat);
                value
            });
            let kept = if replicated {
                own.filter(|_| character_on_seat && is_persisted_per_owner_slot(record))
            } else {
                own
            };
            if kept.is_none()
                && is_persisted_per_owner_slot(record)
                && (!replicated || !character_on_seat)
            {
                from_boot.insert(name.to_owned());
            }
            let value = kept.or_else(|| record.schema.default.clone());
            match value.clone() {
                Some(value) => record.set_per_seat_value(SINGLE_PLAYER_SEAT, value),
                None => record.clear_per_seat_value(SINGLE_PLAYER_SEAT),
            }
            record.write_value(value);
        } else if replicated {
            record.write_value(record.schema.default.clone());
        }
    }

    let Some(boot) = boot else {
        return Vec::new();
    };
    if !persisted_state_version_is_supported(boot) {
        return vec![unsupported_state_version_warning(boot.version)];
    }
    let authored_by_key = authored_names_by_durable_key(identity, committed_store_slots);
    let mut warnings = Vec::new();
    for (durable_key, persisted_value) in &boot.slots {
        let Some(record) = authored_by_key
            .get(durable_key.as_str())
            .and_then(|name| table.get_mut(name))
        else {
            continue;
        };
        if record.schema.network == ReplicationScope::None || !is_persisted_mod_slot(record) {
            continue;
        }
        let name = authored_by_key[durable_key.as_str()];
        match restored_value(name, record, persisted_value) {
            Ok((value, warning)) => {
                record.write_value(Some(value));
                warnings.extend(warning);
            }
            Err(warning) => warnings.push(warning),
        }
    }
    // Only the rows this restore needs: boot already warned about the rest.
    let needed = PersistedState {
        per_owner: boot
            .per_owner
            .iter()
            .filter(|(durable_key, _)| {
                authored_by_key
                    .get(durable_key.as_str())
                    .is_some_and(|name| from_boot.contains(*name))
            })
            .map(|(durable_key, rows)| (durable_key.clone(), rows.clone()))
            .collect(),
        slots: BTreeMap::new(),
        faction_sentiment: BTreeMap::new(),
        version: boot.version,
    };
    warnings.extend(overlay_per_owner_state(
        table,
        &needed,
        &authored_by_key,
        local_player_id,
        SINGLE_PLAYER_SEAT,
        |name, _| from_boot.contains(name),
    ));
    for name in &from_boot {
        if let Some(record) = table.get_mut(name) {
            let value = record.per_seat_value(SINGLE_PLAYER_SEAT).cloned();
            record.write_value(value);
        }
    }
    warnings
}
