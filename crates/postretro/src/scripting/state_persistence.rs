// State-store persistence encoding, restore lifecycle, and save gating.
// See: context/lib/scripting.md §5 "Durable State Store"

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use postretro_entities::slot_table::{SlotOwnership, SlotRecord, SlotTable, SlotType, SlotValue};
use postretro_foundation::Seat;
use postretro_net::wire::{JoinSeedValue, PlayerClaimId};
use postretro_scripting_core::store_identity::StoreIdentityLedger;

/// Current on-disk state format. Increment only with an explicit migration or invalidation policy.
pub(crate) const CURRENT_STATE_VERSION: u32 = 3;
const OLDEST_SUPPORTED_STATE_VERSION: u32 = 2;
const STATE_FILENAME: &str = "state.json";
pub(crate) const PER_OWNER_SAVE_INTERVAL: Duration = Duration::from_secs(60);

/// Process-lifetime gate for the one-time restore and clean-exit save.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StateStoreLifecycle {
    restore_completed: bool,
    persistence_disabled: bool,
}

impl StateStoreLifecycle {
    pub(crate) fn should_restore_after_mod_init(&self, has_manifest: bool) -> bool {
        has_manifest && !self.restore_completed && !self.persistence_disabled
    }

    pub(crate) fn mark_restore_completed(&mut self) {
        self.restore_completed = true;
    }

    pub(crate) fn can_save(&self) -> bool {
        self.restore_completed && !self.persistence_disabled
    }

    /// Disable persistence for the remainder of this process. Returns whether
    /// this call changed state so callers can emit the unavailable-path warning
    /// exactly once.
    pub(crate) fn disable_persistence(&mut self) -> bool {
        let was_enabled = !self.persistence_disabled;
        self.persistence_disabled = true;
        was_enabled
    }
}

/// Frame-driven cadence for a connected client's private save document.
///
/// The timer is deliberately main-thread-only: state collection and the atomic
/// file replacement occur against the same settled frame snapshot. Connection
/// recovery clears accumulated time without changing whether participation has
/// made the timer eligible to run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PerOwnerSaveTimer {
    elapsed: Duration,
    was_connected: bool,
}

impl PerOwnerSaveTimer {
    pub(crate) fn observe_connection(&mut self, connected: bool) {
        if connected && !self.was_connected {
            self.elapsed = Duration::ZERO;
        }
        self.was_connected = connected;
    }

    /// Advance only during an active participation generation. Returns `true`
    /// once when the save is due, resetting the cadence for the next interval.
    pub(crate) fn advance(&mut self, frame_dt: Duration, participating: bool) -> bool {
        if !participating {
            return false;
        }

        self.elapsed = self.elapsed.saturating_add(frame_dt);
        if self.elapsed < PER_OWNER_SAVE_INTERVAL {
            return false;
        }

        self.elapsed = Duration::ZERO;
        true
    }
}

/// Resolve one mod's state file under the platform data directory. The project
/// name is already the final `postretro` component of `data_dir`; do not add it
/// again here.
pub(crate) fn state_path(mod_id: &str) -> Option<PathBuf> {
    let project_dirs = ProjectDirs::from("", "", "postretro");
    state_path_from_data_dir(project_dirs.as_ref().map(|dirs| dirs.data_dir()), mod_id)
}

fn state_path_from_data_dir(data_dir: Option<&Path>, mod_id: &str) -> Option<PathBuf> {
    data_dir.map(|data_dir| data_dir.join(mod_id).join(STATE_FILENAME))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PersistedState {
    version: u32,
    slots: BTreeMap<String, PersistedValue>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    per_owner: BTreeMap<String, BTreeMap<String, PersistedValue>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum PersistedValue {
    Boolean(bool),
    Number(f64),
    String(String),
    Array(Vec<f64>),
    Unsupported(Value),
}

impl From<JoinSeedValue> for PersistedValue {
    fn from(value: JoinSeedValue) -> Self {
        match value {
            JoinSeedValue::Boolean(value) => Self::Boolean(value),
            JoinSeedValue::Number(value) => Self::Number(value),
            JoinSeedValue::String(value) => Self::String(value),
            JoinSeedValue::Array(value) => Self::Array(value),
        }
    }
}

impl TryFrom<PersistedValue> for JoinSeedValue {
    type Error = ();

    fn try_from(value: PersistedValue) -> Result<Self, Self::Error> {
        match value {
            PersistedValue::Boolean(value) => Ok(Self::Boolean(value)),
            PersistedValue::Number(value) => Ok(Self::Number(value)),
            PersistedValue::String(value) => Ok(Self::String(value)),
            PersistedValue::Array(value) => Ok(Self::Array(value)),
            PersistedValue::Unsupported(_) => Err(()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PersistenceError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub(crate) struct CollectedState {
    pub(crate) state: PersistedState,
    pub(crate) warnings: Vec<String>,
}

/// Build the save document without performing filesystem I/O. Retained live mod
/// slots absent from the latest committed declarations are not save members.
pub(crate) fn collect_persisted_state(
    table: &SlotTable,
    identity: Option<&StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
) -> CollectedState {
    let mut slots = BTreeMap::new();
    let mut warnings = Vec::new();

    for (name, record) in table.iter() {
        if !is_persisted_mod_slot(record) {
            continue;
        }
        if !committed_store_slots.contains(name) {
            continue;
        }

        let Some(value) = record.value.as_ref() else {
            warnings.push(format!(
                "persistent state slot `{name}` has no current value; omitting it"
            ));
            continue;
        };

        let Some(durable_key) = identity.and_then(|ledger| ledger.durable_key(name)) else {
            warnings.push(format!(
                "persistent state slot `{name}` has no durable identity; omitting it"
            ));
            continue;
        };

        match value_for_save(name, record, value) {
            Ok(value) => {
                slots.insert(durable_key.to_string(), value);
            }
            Err(warning) => warnings.push(warning),
        }
    }

    CollectedState {
        state: PersistedState {
            version: CURRENT_STATE_VERSION,
            slots,
            per_owner: BTreeMap::new(),
        },
        warnings,
    }
}

/// Collect one player's persistent per-owner slots. The session-scoped seat is
/// used only to select the live value; the saved key is the durable player id.
pub(crate) fn collect_per_owner_state(
    table: &SlotTable,
    identity: Option<&StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
    local_seat: Seat,
    local_player_id: [u8; 16],
) -> CollectedPerOwnerState {
    let mut per_owner = BTreeMap::new();
    let mut warnings = Vec::new();
    let encoded_player_id = encode_player_claim_id(&PlayerClaimId(local_player_id));

    for (name, record) in table.iter() {
        if !is_persisted_per_owner_slot(record) || !committed_store_slots.contains(name) {
            continue;
        }

        let Some(value) = record.per_seat_value(local_seat) else {
            warnings.push(format!(
                "persistent per-owner state slot `{name}` has no current value; omitting it"
            ));
            continue;
        };

        let Some(durable_key) = identity.and_then(|ledger| ledger.durable_key(name)) else {
            warnings.push(format!(
                "persistent per-owner state slot `{name}` has no durable identity; omitting it"
            ));
            continue;
        };

        match value_for_save(name, record, value) {
            Ok(value) => {
                per_owner
                    .entry(durable_key.to_owned())
                    .or_insert_with(BTreeMap::new)
                    .insert(encoded_player_id.clone(), value);
            }
            Err(warning) => warnings.push(warning),
        }
    }

    CollectedPerOwnerState {
        per_owner,
        warnings,
    }
}

pub(crate) struct CollectedPerOwnerState {
    pub(crate) per_owner: BTreeMap<String, BTreeMap<String, PersistedValue>>,
    pub(crate) warnings: Vec<String>,
}

/// Extract one local player's retained per-owner values for the Control-channel
/// join seed. JSON-only unsupported values cannot cross bitcode, so preserve
/// them on disk but omit them from the wire payload with an actionable warning.
pub(crate) fn join_seed_from_persisted_state(
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

/// Validate and apply an admitted player's join seed through the same durable
/// identity and schema checks used by the restore path. A join seed is stricter
/// about bounded numbers: out-of-range values are rejected rather than clamped
/// so a client cannot turn a stale or malicious value into a host-side write.
pub(crate) fn apply_join_seed(
    table: &mut SlotTable,
    identity: Option<&StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
    seat: Seat,
    slots: BTreeMap<String, JoinSeedValue>,
) -> Vec<String> {
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
            Ok((value, None)) => record.set_per_seat_value(seat, value),
            Err(warning) => warnings.push(format!("join seed {warning}")),
        }
    }

    warnings
}

/// Build the client-only document. It preserves retained player entries while
/// replacing this player's freshly collected values, and never carries globals.
pub(crate) fn collected_per_owner_only_state(
    retained: Option<&PersistedState>,
    collected: BTreeMap<String, BTreeMap<String, PersistedValue>>,
) -> PersistedState {
    let mut per_owner = retained
        .map(|state| state.per_owner.clone())
        .unwrap_or_default();
    for (durable_key, player_values) in collected {
        per_owner
            .entry(durable_key)
            .or_insert_with(BTreeMap::new)
            .extend(player_values);
    }

    PersistedState {
        version: CURRENT_STATE_VERSION,
        // A client must never write the host-authoritative global projection,
        // even if it loaded those entries during an earlier single-player run.
        slots: BTreeMap::new(),
        per_owner,
    }
}

/// Record a successful client-private save in the retained boot document while
/// preserving any global section that document carried.
pub(crate) fn retain_saved_per_owner_state(
    retained: &mut Option<PersistedState>,
    saved: PersistedState,
) {
    if let Some(retained) = retained.as_mut() {
        retained.per_owner = saved.per_owner;
    } else {
        *retained = Some(saved);
    }
}

/// Merge freshly collected entries into a save document without touching its
/// global section. Used by the host clean-exit path, which saves its local
/// player alongside the existing global collector.
pub(crate) fn merge_per_owner_state(
    state: &mut PersistedState,
    per_owner: BTreeMap<String, BTreeMap<String, PersistedValue>>,
) {
    for (durable_key, player_values) in per_owner {
        state
            .per_owner
            .entry(durable_key)
            .or_insert_with(BTreeMap::new)
            .extend(player_values);
    }
}

/// Mirror the client-visible scalar projection of each per-owner slot into the
/// local seat cache. Replication intentionally exposes only this client's
/// scalar value; the cache lets persistence use the same seat-addressed read
/// path as the authoritative host without ever inventing another player's data.
pub(crate) fn sync_client_per_owner_projection(table: &mut SlotTable, local_seat: Seat) {
    for (_, record) in table.iter_mut() {
        if !record.schema.per_owner {
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

/// Overlay a decoded save document onto already-declared slots.
///
/// Invalid entries are left at their current declared/default value and
/// returned as warnings for the caller to log. Ledger rows outside current
/// declaration membership cannot target the add-only live table.
pub(crate) fn overlay_persisted_state(
    table: &mut SlotTable,
    persisted: &PersistedState,
    identity: Option<&StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
    local_player_id: Option<[u8; 16]>,
    local_seat: Seat,
) -> Vec<String> {
    if !(OLDEST_SUPPORTED_STATE_VERSION..=CURRENT_STATE_VERSION).contains(&persisted.version) {
        return vec![format!(
            "state file version {} is not supported (current version is {}); ignoring file",
            persisted.version, CURRENT_STATE_VERSION
        )];
    }

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
    for (durable_key, persisted_value) in &persisted.slots {
        let Some(name) = authored_by_key.get(durable_key.as_str()).copied() else {
            warnings.push(format!(
                "state file contains unknown durable key `{durable_key}`; ignoring it"
            ));
            continue;
        };
        let Some(record) = table.get_mut(name) else {
            warnings.push(format!(
                "state file durable key `{durable_key}` targets unavailable slot `{name}`; ignoring it"
            ));
            continue;
        };

        if !is_persisted_mod_slot(record) {
            warnings.push(format!(
                "state file targets non-persistent, per-owner, readonly, or engine-owned slot `{name}`; ignoring it"
            ));
            continue;
        }

        match restored_value(name, record, persisted_value) {
            Ok((value, warning)) => {
                record.write_value(Some(value));
                if let Some(warning) = warning {
                    warnings.push(warning);
                }
            }
            Err(warning) => warnings.push(warning),
        }
    }

    let Some(local_player_id) = local_player_id else {
        return warnings;
    };
    let local_player_id = PlayerClaimId(local_player_id);
    for (durable_key, player_values) in &persisted.per_owner {
        let Some(name) = authored_by_key.get(durable_key.as_str()).copied() else {
            warnings.push(format!(
                "state file contains unknown durable key `{durable_key}`; ignoring it"
            ));
            continue;
        };
        let Some(record) = table.get_mut(name) else {
            warnings.push(format!(
                "state file durable key `{durable_key}` targets unavailable slot `{name}`; ignoring it"
            ));
            continue;
        };

        if !is_persisted_per_owner_slot(record) {
            warnings.push(format!(
                "state file targets non-persistent, global, readonly, or engine-owned per-owner slot `{name}`; ignoring it"
            ));
            continue;
        }

        for (encoded_player_id, persisted_value) in player_values {
            if decode_player_claim_id(encoded_player_id) != Some(local_player_id) {
                continue;
            }

            match restored_value(name, record, persisted_value) {
                Ok((value, warning)) => {
                    record.set_per_seat_value(local_seat, value);
                    if let Some(warning) = warning {
                        warnings.push(warning);
                    }
                }
                Err(warning) => warnings.push(warning),
            }
        }
    }
    warnings
}

pub(crate) fn load_persisted_state(
    path: &Path,
) -> Result<Option<PersistedState>, PersistenceError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok(Some(serde_json::from_slice(&bytes)?))
}

pub(crate) fn save_persisted_state(
    path: &Path,
    state: &PersistedState,
) -> Result<(), PersistenceError> {
    let bytes = serde_json::to_vec_pretty(state)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = tmp_path_for(path);
    if let Err(error) = fs::write(&tmp_path, bytes) {
        let _ = fs::remove_file(&tmp_path);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(error.into());
    }
    Ok(())
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

fn is_persisted_mod_slot(record: &SlotRecord) -> bool {
    record.schema.persist
        && !record.schema.per_owner
        && !record.schema.readonly
        && record.schema.ownership == SlotOwnership::Mod
}

fn is_persisted_per_owner_slot(record: &SlotRecord) -> bool {
    record.schema.persist
        && record.schema.per_owner
        && !record.schema.readonly
        && record.schema.ownership == SlotOwnership::Mod
}

/// Encode the opaque player claim as the stable 32-character JSON map key.
pub(crate) fn encode_player_claim_id(player_id: &PlayerClaimId) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(player_id.0.len() * 2);
    for byte in player_id.0 {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// Decode a 32-character hexadecimal player-claim map key.
pub(crate) fn decode_player_claim_id(encoded: &str) -> Option<PlayerClaimId> {
    if encoded.len() != 32 {
        return None;
    }

    let mut player_id = [0; 16];
    for (index, pair) in encoded.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        player_id[index] = (high << 4) | low;
    }
    Some(PlayerClaimId(player_id))
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn value_for_save(
    name: &str,
    record: &SlotRecord,
    value: &SlotValue,
) -> Result<PersistedValue, String> {
    match (&record.schema.slot_type, value) {
        (SlotType::Number, SlotValue::Number(number)) if number.is_finite() => {
            Ok(PersistedValue::Number(f64::from(*number)))
        }
        (SlotType::Boolean, SlotValue::Boolean(boolean)) => Ok(PersistedValue::Boolean(*boolean)),
        (SlotType::String, SlotValue::String(string)) => Ok(PersistedValue::String(string.clone())),
        (SlotType::Enum { values }, SlotValue::Enum(value)) if values.contains(value) => {
            Ok(PersistedValue::String(value.clone()))
        }
        (SlotType::Array, SlotValue::Array(values))
            if values.iter().all(|value| value.is_finite()) =>
        {
            Ok(PersistedValue::Array(
                values.iter().copied().map(f64::from).collect(),
            ))
        }
        _ => Err(format!(
            "persistent state slot `{name}` has an invalid current value; omitting it"
        )),
    }
}

fn restored_value(
    name: &str,
    record: &SlotRecord,
    persisted: &PersistedValue,
) -> Result<(SlotValue, Option<String>), String> {
    let mismatch = || {
        format!("state file value for slot `{name}` does not match its declared type; ignoring it")
    };

    match (&record.schema.slot_type, persisted) {
        (SlotType::Number, PersistedValue::Number(number)) => {
            if !number.is_finite() {
                return Err(format!(
                    "state file value for number slot `{name}` is non-finite; ignoring it"
                ));
            }
            let narrowed = *number as f32;
            if !narrowed.is_finite() {
                return Err(format!(
                    "state file value for number slot `{name}` is outside the supported numeric range; ignoring it"
                ));
            }

            if let Some(range) = record.schema.range {
                let clamped = narrowed.clamp(range.min, range.max);
                let warning = (clamped != narrowed).then(|| {
                    format!(
                        "state file value {narrowed} for slot `{name}` is outside [{}, {}]; clamped to {clamped}",
                        range.min, range.max
                    )
                });
                Ok((SlotValue::Number(clamped), warning))
            } else {
                Ok((SlotValue::Number(narrowed), None))
            }
        }
        (SlotType::Boolean, PersistedValue::Boolean(boolean)) => {
            Ok((SlotValue::Boolean(*boolean), None))
        }
        (SlotType::String, PersistedValue::String(string)) => {
            Ok((SlotValue::String(string.clone()), None))
        }
        (SlotType::Enum { values }, PersistedValue::String(value)) => {
            if values.contains(value) {
                Ok((SlotValue::Enum(value.clone()), None))
            } else {
                Err(format!(
                    "state file enum value `{value}` for slot `{name}` is not declared; ignoring it"
                ))
            }
        }
        (SlotType::Array, PersistedValue::Array(values)) => {
            let mut narrowed = Vec::with_capacity(values.len());
            for value in values {
                let element = *value as f32;
                if !value.is_finite() || !element.is_finite() {
                    return Err(format!(
                        "state file array for slot `{name}` contains a non-finite or unsupported number; ignoring it"
                    ));
                }
                narrowed.push(element);
            }
            Ok((SlotValue::Array(narrowed), None))
        }
        _ => Err(mismatch()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::slot_table::{NumericRange, ReplicationScope, SlotSchema};
    use tempfile::tempdir;

    #[test]
    fn lifecycle_requires_successful_manifest_and_one_restore_attempt_before_save() {
        let mut lifecycle = StateStoreLifecycle::default();
        assert!(!lifecycle.should_restore_after_mod_init(false));
        assert!(!lifecycle.can_save());

        assert!(lifecycle.should_restore_after_mod_init(true));
        assert!(!lifecycle.can_save());
        lifecycle.mark_restore_completed();

        assert!(lifecycle.can_save());
        assert!(!lifecycle.should_restore_after_mod_init(true));
        assert!(!lifecycle.should_restore_after_mod_init(false));
    }

    fn mod_slot(
        slot_type: SlotType,
        default: SlotValue,
        persist: bool,
        range: Option<NumericRange>,
    ) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type,
            default: Some(default),
            range,
            persist,
            readonly: false,
            ownership: SlotOwnership::Mod,
            network: ReplicationScope::None,
            per_owner: false,
            accumulate: None,
        })
    }

    fn per_owner_mod_slot(persist: bool) -> SlotRecord {
        per_owner_mod_slot_with_type(SlotType::Number, SlotValue::Number(0.0), persist, None)
    }

    fn per_owner_mod_slot_with_type(
        slot_type: SlotType,
        default: SlotValue,
        persist: bool,
        range: Option<NumericRange>,
    ) -> SlotRecord {
        let mut record = mod_slot(slot_type, default, persist, range);
        record.schema.per_owner = true;
        record
    }

    fn declare_fixture(table: &mut SlotTable) {
        table
            .insert_namespace(
                "game",
                vec![
                    (
                        "score".to_string(),
                        mod_slot(
                            SlotType::Number,
                            SlotValue::Number(10.0),
                            true,
                            Some(NumericRange {
                                min: 0.0,
                                max: 100.0,
                            }),
                        ),
                    ),
                    (
                        "mode".to_string(),
                        mod_slot(
                            SlotType::Enum {
                                values: vec!["normal".to_string(), "hard".to_string()],
                            },
                            SlotValue::Enum("normal".to_string()),
                            true,
                            None,
                        ),
                    ),
                    (
                        "enabled".to_string(),
                        mod_slot(SlotType::Boolean, SlotValue::Boolean(false), true, None),
                    ),
                    (
                        "label".to_string(),
                        mod_slot(
                            SlotType::String,
                            SlotValue::String("default".to_string()),
                            true,
                            None,
                        ),
                    ),
                    (
                        "curve".to_string(),
                        mod_slot(
                            SlotType::Array,
                            SlotValue::Array(vec![0.0, 1.0]),
                            true,
                            None,
                        ),
                    ),
                    (
                        "scratch".to_string(),
                        mod_slot(SlotType::Boolean, SlotValue::Boolean(false), false, None),
                    ),
                    (
                        "xp".to_string(),
                        per_owner_mod_slot_with_type(
                            SlotType::Number,
                            SlotValue::Number(0.0),
                            true,
                            Some(NumericRange {
                                min: 0.0,
                                max: 100.0,
                            }),
                        ),
                    ),
                    (
                        "rank".to_string(),
                        per_owner_mod_slot_with_type(
                            SlotType::Enum {
                                values: vec!["rookie".to_string(), "veteran".to_string()],
                            },
                            SlotValue::Enum("rookie".to_string()),
                            true,
                            None,
                        ),
                    ),
                    (
                        "session_xp".to_string(),
                        per_owner_mod_slot_with_type(
                            SlotType::Number,
                            SlotValue::Number(0.0),
                            false,
                            None,
                        ),
                    ),
                ],
            )
            .unwrap();
    }

    fn fixture_identity() -> StoreIdentityLedger {
        StoreIdentityLedger {
            version: 1,
            slots: BTreeMap::from([
                ("game.score".to_string(), "k0000000000000001".to_string()),
                ("game.mode".to_string(), "k0000000000000002".to_string()),
                ("game.enabled".to_string(), "k0000000000000003".to_string()),
                ("game.label".to_string(), "k0000000000000004".to_string()),
                ("game.curve".to_string(), "k0000000000000005".to_string()),
                ("game.xp".to_string(), "k0000000000000010".to_string()),
                ("game.rank".to_string(), "k0000000000000011".to_string()),
                (
                    "game.session_xp".to_string(),
                    "k0000000000000012".to_string(),
                ),
            ]),
        }
    }

    fn player_id(byte: u8) -> [u8; 16] {
        [byte; 16]
    }

    fn identity_membership(identity: Option<&StoreIdentityLedger>) -> BTreeSet<String> {
        identity
            .into_iter()
            .flat_map(|identity| identity.slots.keys().cloned())
            .collect()
    }

    #[test]
    fn persisted_state_roundtrips_per_owner_entries() {
        let persisted = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([(
                "kglobal0000000001".to_string(),
                PersistedValue::Boolean(true),
            )]),
            per_owner: BTreeMap::from([(
                "kplayer0000000001".to_string(),
                BTreeMap::from([(
                    "00112233445566778899aabbccddeeff".to_string(),
                    PersistedValue::Number(42.0),
                )]),
            )]),
        };

        let serialized = serde_json::to_vec(&persisted).unwrap();
        let restored: PersistedState = serde_json::from_slice(&serialized).unwrap();

        assert_eq!(restored, persisted);
    }

    #[test]
    fn version_two_document_defaults_per_owner_and_restores_global_slots() {
        let persisted: PersistedState = serde_json::from_value(serde_json::json!({
            "version": 2,
            "slots": { "k0000000000000001": 42.0 }
        }))
        .unwrap();
        assert!(persisted.per_owner.is_empty());

        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        assert!(
            overlay_persisted_state(
                &mut table,
                &persisted,
                Some(&fixture_identity()),
                &identity_membership(Some(&fixture_identity())),
                None,
                Seat(0),
            )
            .is_empty()
        );
        assert_eq!(
            table.get("game.score").unwrap().value,
            Some(SlotValue::Number(42.0))
        );
    }

    #[test]
    fn player_claim_id_hex_roundtrips() {
        let player_id = PlayerClaimId([
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ]);

        let encoded = encode_player_claim_id(&player_id);
        assert_eq!(encoded, "00112233445566778899aabbccddeeff");
        assert_eq!(decode_player_claim_id(&encoded), Some(player_id));
        assert_eq!(
            decode_player_claim_id("00112233445566778899AABBCCDDEEFF"),
            Some(player_id)
        );
        assert!(decode_player_claim_id("00112233445566778899aabbccddee").is_none());
        assert!(decode_player_claim_id("00112233445566778899aabbccddeefg").is_none());
    }

    #[test]
    fn persistence_filters_keep_per_owner_slots_out_of_global_state() {
        let global_persistent = mod_slot(SlotType::Boolean, SlotValue::Boolean(false), true, None);
        let per_owner_persistent = per_owner_mod_slot(true);
        let per_owner_runtime_only = per_owner_mod_slot(false);

        assert!(is_persisted_mod_slot(&global_persistent));
        assert!(!is_persisted_per_owner_slot(&global_persistent));
        assert!(!is_persisted_mod_slot(&per_owner_persistent));
        assert!(is_persisted_per_owner_slot(&per_owner_persistent));
        assert!(!is_persisted_mod_slot(&per_owner_runtime_only));
        assert!(!is_persisted_per_owner_slot(&per_owner_runtime_only));
    }

    #[test]
    fn persisted_slots_roundtrip_over_fresh_declarations() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");

        let mut source = SlotTable::new();
        declare_fixture(&mut source);
        let identity = fixture_identity();
        source.get_mut("game.score").unwrap().value = Some(SlotValue::Number(42.0));
        source.get_mut("game.mode").unwrap().value = Some(SlotValue::Enum("hard".to_string()));
        source.get_mut("game.enabled").unwrap().value = Some(SlotValue::Boolean(true));
        source.get_mut("game.label").unwrap().value =
            Some(SlotValue::String("continued".to_string()));
        source.get_mut("game.curve").unwrap().value = Some(SlotValue::Array(vec![0.25, 0.75, 1.0]));
        source.get_mut("game.scratch").unwrap().value = Some(SlotValue::Boolean(true));

        let membership = identity_membership(Some(&identity));
        let collected = collect_persisted_state(&source, Some(&identity), &membership);
        assert!(collected.warnings.is_empty());
        assert_eq!(
            collected.state.slots.get("k0000000000000002"),
            Some(&PersistedValue::String("hard".to_string()))
        );
        assert!(!collected.state.slots.contains_key("game.scratch"));
        save_persisted_state(&path, &collected.state).unwrap();

        let mut fresh = SlotTable::new();
        declare_fixture(&mut fresh);
        let loaded = load_persisted_state(&path).unwrap().unwrap();
        assert!(
            overlay_persisted_state(
                &mut fresh,
                &loaded,
                Some(&identity),
                &membership,
                None,
                Seat(0)
            )
            .is_empty()
        );

        assert_eq!(
            fresh.get("game.score").unwrap().value,
            Some(SlotValue::Number(42.0))
        );
        assert_eq!(
            fresh.get("game.mode").unwrap().value,
            Some(SlotValue::Enum("hard".to_string()))
        );
        assert_eq!(
            fresh.get("game.enabled").unwrap().value,
            Some(SlotValue::Boolean(true))
        );
        assert_eq!(
            fresh.get("game.label").unwrap().value,
            Some(SlotValue::String("continued".to_string()))
        );
        assert_eq!(
            fresh.get("game.curve").unwrap().value,
            Some(SlotValue::Array(vec![0.25, 0.75, 1.0]))
        );
        assert_eq!(
            fresh.get("game.scratch").unwrap().value,
            Some(SlotValue::Boolean(false))
        );
    }

    #[test]
    fn overlay_ignores_unknown_mismatched_and_invalid_enum_entries() {
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        let identity = fixture_identity();
        let persisted = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([
                (
                    "k0000000000000001".to_string(),
                    PersistedValue::String("many".into()),
                ),
                (
                    "k0000000000000002".to_string(),
                    PersistedValue::String("nightmare".into()),
                ),
                ("kffffffffffffffff".to_string(), PersistedValue::Number(1.0)),
            ]),
            per_owner: BTreeMap::new(),
        };

        let warnings = overlay_persisted_state(
            &mut table,
            &persisted,
            Some(&identity),
            &identity_membership(Some(&identity)),
            None,
            Seat(0),
        );
        assert_eq!(warnings.len(), 3);
        assert_eq!(
            table.get("game.score").unwrap().value,
            Some(SlotValue::Number(10.0))
        );
        assert_eq!(
            table.get("game.mode").unwrap().value,
            Some(SlotValue::Enum("normal".to_string()))
        );
    }

    #[test]
    fn overlay_ignores_bad_version_and_non_finite_rust_values() {
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        let identity = fixture_identity();
        let bad_version = PersistedState {
            version: CURRENT_STATE_VERSION + 1,
            slots: BTreeMap::from([(
                "k0000000000000001".to_string(),
                PersistedValue::Number(99.0),
            )]),
            per_owner: BTreeMap::new(),
        };
        assert_eq!(
            overlay_persisted_state(
                &mut table,
                &bad_version,
                Some(&identity),
                &identity_membership(Some(&identity)),
                None,
                Seat(0),
            )
            .len(),
            1
        );
        assert_eq!(
            table.get("game.score").unwrap().value,
            Some(SlotValue::Number(10.0))
        );

        let non_finite = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([(
                "k0000000000000001".to_string(),
                PersistedValue::Number(f64::NAN),
            )]),
            per_owner: BTreeMap::new(),
        };
        assert_eq!(
            overlay_persisted_state(
                &mut table,
                &non_finite,
                Some(&identity),
                &identity_membership(Some(&identity)),
                None,
                Seat(0),
            )
            .len(),
            1
        );
        assert_eq!(
            table.get("game.score").unwrap().value,
            Some(SlotValue::Number(10.0))
        );

        let non_finite_array = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([(
                "k0000000000000005".to_string(),
                PersistedValue::Array(vec![0.0, f64::INFINITY]),
            )]),
            per_owner: BTreeMap::new(),
        };
        assert_eq!(
            overlay_persisted_state(
                &mut table,
                &non_finite_array,
                Some(&identity),
                &identity_membership(Some(&identity)),
                None,
                Seat(0),
            )
            .len(),
            1
        );
        assert_eq!(
            table.get("game.curve").unwrap().value,
            Some(SlotValue::Array(vec![0.0, 1.0]))
        );
    }

    #[test]
    fn empty_persist_set_writes_empty_slots_map_and_missing_file_is_ok() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.json");
        let table = SlotTable::new();
        let collected = collect_persisted_state(&table, None, &BTreeSet::new());

        assert!(collected.state.slots.is_empty());
        assert!(load_persisted_state(&path).unwrap().is_none());

        save_persisted_state(&path, &collected.state).unwrap();
        let json: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(json["version"], CURRENT_STATE_VERSION);
        assert_eq!(json["slots"], serde_json::json!({}));
        assert!(json.get("per_owner").is_none());
    }

    #[test]
    fn overlay_clamps_finite_out_of_range_numbers() {
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        let identity = fixture_identity();
        let persisted = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([(
                "k0000000000000001".to_string(),
                PersistedValue::Number(500.0),
            )]),
            per_owner: BTreeMap::new(),
        };

        let warnings = overlay_persisted_state(
            &mut table,
            &persisted,
            Some(&identity),
            &identity_membership(Some(&identity)),
            None,
            Seat(0),
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            table.get("game.score").unwrap().value,
            Some(SlotValue::Number(100.0))
        );
    }

    #[test]
    fn per_owner_roundtrip_restores_only_the_matching_player_and_preserves_others() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        let identity = fixture_identity();
        let membership = identity_membership(Some(&identity));
        let local_id = player_id(0x11);
        let other_id = player_id(0x22);
        let local_seat = Seat(7);

        let mut source = SlotTable::new();
        declare_fixture(&mut source);
        let xp = source.get_mut("game.xp").unwrap();
        xp.set_per_seat_value(local_seat, SlotValue::Number(42.0));
        xp.set_per_seat_value(Seat(8), SlotValue::Number(99.0));

        let collected =
            collect_per_owner_state(&source, Some(&identity), &membership, local_seat, local_id);
        assert!(collected.warnings.is_empty());
        assert_eq!(
            collected.per_owner,
            BTreeMap::from([
                (
                    "k0000000000000010".to_string(),
                    BTreeMap::from([(
                        encode_player_claim_id(&PlayerClaimId(local_id)),
                        PersistedValue::Number(42.0),
                    )]),
                ),
                (
                    "k0000000000000011".to_string(),
                    BTreeMap::from([(
                        encode_player_claim_id(&PlayerClaimId(local_id)),
                        PersistedValue::String("rookie".to_string()),
                    )]),
                ),
            ]),
            "collection reads only the local session seat, keyed by the durable player id"
        );

        let retained = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([(
                "k0000000000000001".to_string(),
                PersistedValue::Number(88.0),
            )]),
            per_owner: BTreeMap::from([(
                "k0000000000000010".to_string(),
                BTreeMap::from([(
                    encode_player_claim_id(&PlayerClaimId(other_id)),
                    PersistedValue::Number(17.0),
                )]),
            )]),
        };
        let client_document = collected_per_owner_only_state(Some(&retained), collected.per_owner);
        assert!(
            client_document.slots.is_empty(),
            "clients never save globals"
        );
        assert_eq!(
            client_document.per_owner["k0000000000000010"]
                [&encode_player_claim_id(&PlayerClaimId(other_id))],
            PersistedValue::Number(17.0),
            "the retained document keeps another player's entry"
        );
        save_persisted_state(&path, &client_document).unwrap();

        let loaded = load_persisted_state(&path).unwrap().unwrap();
        let mut fresh = SlotTable::new();
        declare_fixture(&mut fresh);
        assert!(
            overlay_persisted_state(
                &mut fresh,
                &loaded,
                Some(&identity),
                &membership,
                Some(local_id),
                local_seat,
            )
            .is_empty()
        );
        assert_eq!(
            fresh.get("game.xp").unwrap().per_seat_value(local_seat),
            Some(&SlotValue::Number(42.0))
        );
        assert_eq!(
            fresh.get("game.xp").unwrap().per_seat_value(Seat(8)),
            Some(&SlotValue::Number(0.0)),
            "the other player's retained entry is not loaded into this session"
        );
    }

    #[test]
    fn per_owner_overlay_applies_global_validation_and_skips_runtime_only_slots() {
        let identity = fixture_identity();
        let membership = identity_membership(Some(&identity));
        let local_id = player_id(0x33);
        let local_key = encode_player_claim_id(&PlayerClaimId(local_id));
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        let persisted = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::new(),
            per_owner: BTreeMap::from([
                (
                    "k0000000000000010".to_string(),
                    BTreeMap::from([(local_key.clone(), PersistedValue::Number(500.0))]),
                ),
                (
                    "k0000000000000011".to_string(),
                    BTreeMap::from([(local_key.clone(), PersistedValue::String("legend".into()))]),
                ),
                (
                    "k0000000000000012".to_string(),
                    BTreeMap::from([(local_key, PersistedValue::Number(99.0))]),
                ),
            ]),
        };

        let warnings = overlay_persisted_state(
            &mut table,
            &persisted,
            Some(&identity),
            &membership,
            Some(local_id),
            Seat(3),
        );
        assert_eq!(
            warnings.len(),
            3,
            "clamp, invalid enum, and runtime-only slot warn"
        );
        assert_eq!(
            table.get("game.xp").unwrap().per_seat_value(Seat(3)),
            Some(&SlotValue::Number(100.0)),
            "per-owner numeric restore clamps through the shared validator"
        );
        assert_eq!(
            table.get("game.rank").unwrap().per_seat_value(Seat(3)),
            Some(&SlotValue::Enum("rookie".to_string())),
            "invalid enum values retain the declared default"
        );
        assert_eq!(
            table
                .get("game.session_xp")
                .unwrap()
                .per_seat_value(Seat(3)),
            Some(&SlotValue::Number(0.0)),
            "per-owner but non-persistent slots never restore"
        );

        let wrong_type = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::new(),
            per_owner: BTreeMap::from([(
                "k0000000000000010".to_string(),
                BTreeMap::from([(
                    encode_player_claim_id(&PlayerClaimId(local_id)),
                    PersistedValue::String("not-a-number".into()),
                )]),
            )]),
        };
        assert_eq!(
            overlay_persisted_state(
                &mut table,
                &wrong_type,
                Some(&identity),
                &membership,
                Some(local_id),
                Seat(3),
            )
            .len(),
            1
        );
        assert_eq!(
            table.get("game.xp").unwrap().per_seat_value(Seat(3)),
            Some(&SlotValue::Number(100.0)),
            "a type mismatch does not replace the validated live value"
        );
    }

    #[test]
    fn client_private_document_merges_retained_entries_without_global_slots() {
        let local_id = player_id(0x44);
        let other_id = player_id(0x55);
        let retained = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([("k0000000000000001".to_string(), PersistedValue::Number(6.0))]),
            per_owner: BTreeMap::from([(
                "k0000000000000010".to_string(),
                BTreeMap::from([
                    (
                        encode_player_claim_id(&PlayerClaimId(local_id)),
                        PersistedValue::Number(3.0),
                    ),
                    (
                        encode_player_claim_id(&PlayerClaimId(other_id)),
                        PersistedValue::Number(9.0),
                    ),
                ]),
            )]),
        };
        let saved = collected_per_owner_only_state(
            Some(&retained),
            BTreeMap::from([(
                "k0000000000000010".to_string(),
                BTreeMap::from([(
                    encode_player_claim_id(&PlayerClaimId(local_id)),
                    PersistedValue::Number(12.0),
                )]),
            )]),
        );

        assert!(saved.slots.is_empty());
        assert_eq!(
            saved.per_owner["k0000000000000010"][&encode_player_claim_id(&PlayerClaimId(local_id))],
            PersistedValue::Number(12.0)
        );
        assert_eq!(
            saved.per_owner["k0000000000000010"][&encode_player_claim_id(&PlayerClaimId(other_id))],
            PersistedValue::Number(9.0)
        );

        let mut retained = Some(retained);
        retain_saved_per_owner_state(&mut retained, saved);
        let retained = retained.unwrap();
        assert_eq!(
            retained.slots.get("k0000000000000001"),
            Some(&PersistedValue::Number(6.0)),
            "the retained boot document keeps globals in memory even though the client file does not"
        );
    }

    #[test]
    fn client_projection_populates_the_local_seat_cache_without_global_fallback() {
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        table
            .get_mut("game.xp")
            .unwrap()
            .write_value(Some(SlotValue::Number(25.0)));
        table
            .get_mut("game.score")
            .unwrap()
            .write_value(Some(SlotValue::Number(88.0)));

        sync_client_per_owner_projection(&mut table, Seat(4));

        assert_eq!(
            table.get("game.xp").unwrap().per_seat_value(Seat(4)),
            Some(&SlotValue::Number(25.0))
        );
        assert_eq!(
            table.get("game.score").unwrap().value,
            Some(SlotValue::Number(88.0)),
            "the helper does not route globals through a seat map"
        );
    }

    #[test]
    fn periodic_client_save_timer_is_participation_gated_and_resets_on_reconnect() {
        let mut timer = PerOwnerSaveTimer::default();
        timer.observe_connection(true);
        assert!(!timer.advance(Duration::from_secs(59), true));
        assert!(timer.advance(Duration::from_secs(1), true));
        assert!(timer.advance(Duration::from_secs(60), true));

        assert!(!timer.advance(Duration::from_secs(59), true));
        assert!(!timer.advance(Duration::from_secs(120), false));
        assert!(timer.advance(Duration::from_secs(1), true));

        assert!(!timer.advance(Duration::from_secs(30), true));
        timer.observe_connection(false);
        timer.observe_connection(true);
        assert!(!timer.advance(Duration::from_secs(59), true));
        assert!(timer.advance(Duration::from_secs(1), true));
    }

    #[test]
    fn save_and_overlay_reject_readonly_or_non_persistent_targets() {
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        let mut identity = fixture_identity();
        table
            .insert(
                "game.locked".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::String,
                    default: Some(SlotValue::String("default".to_string())),
                    range: None,
                    persist: true,
                    readonly: true,
                    ownership: SlotOwnership::Mod,
                    network: ReplicationScope::None,
                    per_owner: false,
                    accumulate: None,
                }),
            )
            .unwrap();

        identity
            .slots
            .insert("game.locked".to_string(), "k0000000000000006".to_string());
        identity
            .slots
            .insert("game.scratch".to_string(), "k0000000000000007".to_string());
        identity
            .slots
            .insert("player.health".to_string(), "k0000000000000008".to_string());
        let membership = identity_membership(Some(&identity));
        let collected = collect_persisted_state(&table, Some(&identity), &membership);
        assert!(!collected.state.slots.contains_key("game.locked"));
        assert!(!collected.state.slots.contains_key("game.scratch"));

        let persisted = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([
                (
                    "k0000000000000006".to_string(),
                    PersistedValue::String("changed".to_string()),
                ),
                (
                    "k0000000000000007".to_string(),
                    PersistedValue::Boolean(true),
                ),
                ("k0000000000000008".to_string(), PersistedValue::Number(1.0)),
            ]),
            per_owner: BTreeMap::new(),
        };
        let warnings = overlay_persisted_state(
            &mut table,
            &persisted,
            Some(&identity),
            &membership,
            None,
            Seat(0),
        );

        assert_eq!(warnings.len(), 3);
        assert_eq!(
            table.get("game.locked").unwrap().value,
            Some(SlotValue::String("default".to_string()))
        );
        assert_eq!(
            table.get("game.scratch").unwrap().value,
            Some(SlotValue::Boolean(false))
        );
        assert_eq!(table.get("player.health").unwrap().value, None);
    }

    #[test]
    fn preserved_durable_key_restores_after_an_authored_slot_rename() {
        let mut before_rename = SlotTable::new();
        before_rename
            .insert_namespace(
                "story",
                vec![(
                    "old_score".to_string(),
                    mod_slot(SlotType::Number, SlotValue::Number(0.0), true, None),
                )],
            )
            .unwrap();
        before_rename.get_mut("story.old_score").unwrap().value = Some(SlotValue::Number(42.0));
        let before_identity = StoreIdentityLedger {
            version: 1,
            slots: BTreeMap::from([(
                "story.old_score".to_string(),
                "k0123456789abcdef".to_string(),
            )]),
        };
        let persisted = collect_persisted_state(
            &before_rename,
            Some(&before_identity),
            &identity_membership(Some(&before_identity)),
        )
        .state;

        let mut after_rename = SlotTable::new();
        after_rename
            .insert_namespace(
                "story",
                vec![(
                    "score".to_string(),
                    mod_slot(SlotType::Number, SlotValue::Number(0.0), true, None),
                )],
            )
            .unwrap();
        let renamed_identity = StoreIdentityLedger {
            version: 1,
            slots: BTreeMap::from([("story.score".to_string(), "k0123456789abcdef".to_string())]),
        };

        assert!(
            overlay_persisted_state(
                &mut after_rename,
                &persisted,
                Some(&renamed_identity),
                &identity_membership(Some(&renamed_identity)),
                None,
                Seat(0),
            )
            .is_empty()
        );
        assert_eq!(
            after_rename.get("story.score").unwrap().value,
            Some(SlotValue::Number(42.0))
        );
    }

    #[test]
    fn deleted_ledger_entry_discards_saved_value_by_its_durable_key() {
        let mut table = SlotTable::new();
        table
            .insert_namespace(
                "story",
                vec![(
                    "score".to_string(),
                    mod_slot(SlotType::Number, SlotValue::Number(0.0), true, None),
                )],
            )
            .unwrap();
        let persisted = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([(
                "k0123456789abcdef".to_string(),
                PersistedValue::Number(42.0),
            )]),
            per_owner: BTreeMap::new(),
        };

        let warnings = overlay_persisted_state(
            &mut table,
            &persisted,
            None,
            &BTreeSet::from(["story.score".to_string()]),
            None,
            Seat(0),
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("k0123456789abcdef"));
        assert_eq!(
            table.get("story.score").unwrap().value,
            Some(SlotValue::Number(0.0))
        );
    }

    #[test]
    fn v1_document_is_ignored_after_the_durable_key_format_bump() {
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        let warnings = overlay_persisted_state(
            &mut table,
            &PersistedState {
                version: 1,
                slots: BTreeMap::from([("game.score".to_string(), PersistedValue::Number(42.0))]),
                per_owner: BTreeMap::new(),
            },
            Some(&fixture_identity()),
            &identity_membership(Some(&fixture_identity())),
            None,
            Seat(0),
        );

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("version 1"));
        assert_eq!(
            table.get("game.score").unwrap().value,
            Some(SlotValue::Number(10.0))
        );
    }

    #[test]
    fn platform_state_paths_are_per_mod_without_double_nesting_postretro() {
        let data_dir = Path::new("/tmp/postretro-test-data/postretro");
        let first = state_path_from_data_dir(Some(data_dir), "first.mod").unwrap();
        let second = state_path_from_data_dir(Some(data_dir), "second.mod").unwrap();

        assert_ne!(first, second);
        assert_eq!(first, data_dir.join("first.mod/state.json"));
        assert_eq!(
            first
                .components()
                .filter(|component| component.as_os_str() == "postretro")
                .count(),
            1
        );
        assert!(
            state_path_from_data_dir(None, "first.mod").is_none(),
            "an unavailable platform data directory must not fall back to cwd"
        );
    }

    // Regression: the add-only table and retained ledger kept saving a slot
    // removed by the latest successful manifest commit.
    #[test]
    fn committed_declaration_membership_filters_retained_live_slots() {
        let mut table = SlotTable::new();
        table
            .insert_namespace(
                "old",
                vec![(
                    "score".to_string(),
                    mod_slot(SlotType::Number, SlotValue::Number(7.0), true, None),
                )],
            )
            .unwrap();
        table
            .insert_namespace(
                "current",
                vec![(
                    "score".to_string(),
                    mod_slot(SlotType::Number, SlotValue::Number(11.0), true, None),
                )],
            )
            .unwrap();
        let identity = StoreIdentityLedger {
            version: 1,
            slots: BTreeMap::from([
                ("old.score".to_string(), "k0123456789abcdef".to_string()),
                ("current.score".to_string(), "kfedcba9876543210".to_string()),
            ]),
        };
        let current_membership = BTreeSet::from(["current.score".to_string()]);

        let collected = collect_persisted_state(&table, Some(&identity), &current_membership).state;
        assert_eq!(collected.slots.len(), 1);
        assert_eq!(
            collected.slots.get("kfedcba9876543210"),
            Some(&PersistedValue::Number(11.0))
        );
        assert!(!collected.slots.contains_key("k0123456789abcdef"));

        let after_no_start = collect_persisted_state(&table, Some(&identity), &BTreeSet::new());
        assert!(
            after_no_start.state.slots.is_empty(),
            "an empty successful commit must not save through stale identity entries"
        );
    }

    #[test]
    fn save_uses_a_sibling_temp_file_and_retained_snapshot_key() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(&path, b"old-state").unwrap();
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        table.get_mut("game.score").unwrap().value = Some(SlotValue::Number(7.0));
        let snapshot = fixture_identity();
        let mut hand_edited = snapshot.clone();
        hand_edited
            .slots
            .insert("game.score".to_string(), "kffffffffffffffff".to_string());

        let collected = collect_persisted_state(
            &table,
            Some(&snapshot),
            &identity_membership(Some(&snapshot)),
        );
        assert!(collected.state.slots.contains_key("k0000000000000001"));
        assert!(
            !collected
                .state
                .slots
                .contains_key(hand_edited.durable_key("game.score").unwrap())
        );
        save_persisted_state(&path, &collected.state).unwrap();

        assert!(!tmp_path_for(&path).exists());
        let document: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(document["slots"]["k0000000000000001"], 7.0);
    }

    #[test]
    fn join_seed_uses_only_the_local_players_retained_values() {
        let local_id = player_id(0x11);
        let other_id = player_id(0x22);
        let persisted = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::new(),
            per_owner: BTreeMap::from([
                (
                    "k0000000000000010".to_string(),
                    BTreeMap::from([
                        (
                            encode_player_claim_id(&PlayerClaimId(local_id)),
                            PersistedValue::Number(42.0),
                        ),
                        (
                            encode_player_claim_id(&PlayerClaimId(other_id)),
                            PersistedValue::Number(99.0),
                        ),
                    ]),
                ),
                (
                    "k0000000000000011".to_string(),
                    BTreeMap::from([(
                        encode_player_claim_id(&PlayerClaimId(local_id)),
                        PersistedValue::Unsupported(serde_json::json!({ "legacy": true })),
                    )]),
                ),
            ]),
        };

        assert_eq!(
            join_seed_from_persisted_state(Some(&persisted), Some(local_id)),
            BTreeMap::from([("k0000000000000010".to_string(), JoinSeedValue::Number(42.0),)])
        );
        assert!(join_seed_from_persisted_state(None, Some(local_id)).is_empty());
        assert!(join_seed_from_persisted_state(Some(&persisted), None).is_empty());
    }

    #[test]
    fn join_seed_applies_only_valid_persistent_per_owner_entries() {
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        let identity = fixture_identity();
        let membership = identity_membership(Some(&identity));

        let warnings = apply_join_seed(
            &mut table,
            Some(&identity),
            &membership,
            Seat(7),
            BTreeMap::from([
                ("k0000000000000010".to_string(), JoinSeedValue::Number(42.0)),
                ("k0000000000000011".to_string(), JoinSeedValue::Number(7.0)),
                ("k0000000000000012".to_string(), JoinSeedValue::Number(5.0)),
                ("kmadeup000000000".to_string(), JoinSeedValue::Boolean(true)),
            ]),
        );

        assert_eq!(warnings.len(), 3);
        assert_eq!(
            table.get("game.xp").unwrap().per_seat_value(Seat(7)),
            Some(&SlotValue::Number(42.0))
        );
        assert_eq!(
            table.get("game.rank").unwrap().per_seat_value(Seat(7)),
            Some(&SlotValue::Enum("rookie".to_string()))
        );
        assert_eq!(
            table
                .get("game.session_xp")
                .unwrap()
                .per_seat_value(Seat(7)),
            Some(&SlotValue::Number(0.0))
        );
    }

    #[test]
    fn join_seed_rejects_out_of_range_numbers_instead_of_clamping_them() {
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        let identity = fixture_identity();
        let membership = identity_membership(Some(&identity));

        let warnings = apply_join_seed(
            &mut table,
            Some(&identity),
            &membership,
            Seat(7),
            BTreeMap::from([(
                "k0000000000000010".to_string(),
                JoinSeedValue::Number(500.0),
            )]),
        );

        assert_eq!(warnings.len(), 1);
        assert_eq!(
            table.get("game.xp").unwrap().per_seat_value(Seat(7)),
            Some(&SlotValue::Number(0.0))
        );
    }
}
