// State-store persistence encoding, restore lifecycle, and save gating.
// See: context/lib/scripting.md §5 "Durable State Store"

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use postretro_entities::data_registry::{
    DEFAULT_ENEMY_FACTION_INDEX, FactionRegistry, FactionSentimentState, PLAYER_FACTION_INDEX,
};
use postretro_entities::slot_table::{
    ReplicationScope, SlotOwnership, SlotRecord, SlotTable, SlotType, SlotValue,
};
use postretro_foundation::Seat;
use postretro_net::wire::{JoinSeedValue, PlayerClaimId};
use postretro_scripting_core::store_identity::StoreIdentityLedger;

mod client_session;
mod join_seed;
mod ledger;
#[cfg(test)]
mod tests;

pub use client_session::*;
pub use join_seed::*;
pub use ledger::{
    project_seat_per_owner_values, reset_mod_slots_for_reload, run_scoped_durable_keys,
    split_world_and_run,
};

/// Current on-disk state format. Increment only with an explicit migration or invalidation policy.
pub(crate) const CURRENT_STATE_VERSION: u32 = 4;
const OLDEST_SUPPORTED_STATE_VERSION: u32 = 2;
const STATE_FILENAME: &str = "state.json";
pub(crate) const PER_OWNER_SAVE_INTERVAL: Duration = Duration::from_secs(60);
// Authored factions cannot use the `@postretro.` namespace. These stable keys
// make the two engine-only indices durable without pretending they are authored.
const PERSISTED_PLAYER_FACTION_KEY: &str = "@postretro.player";
const PERSISTED_DEFAULT_ENEMY_FACTION_KEY: &str = "@postretro.default-enemy";

/// Process-lifetime gate for the one-time restore and clean-exit save.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StateStoreLifecycle {
    restore_completed: bool,
    persistence_disabled: bool,
}

impl StateStoreLifecycle {
    pub fn should_restore_after_mod_init(&self, has_manifest: bool) -> bool {
        has_manifest && !self.restore_completed && !self.persistence_disabled
    }

    pub fn mark_restore_completed(&mut self) {
        self.restore_completed = true;
    }

    pub fn can_save(&self) -> bool {
        self.restore_completed && !self.persistence_disabled
    }

    /// Whether the store may be snapshotted in memory: the boot restore has
    /// run (or found persistence unavailable), so the store holds this run's
    /// values rather than declared defaults awaiting a restore.
    pub fn can_collect(&self) -> bool {
        self.restore_completed
    }

    /// Disable persistence for the remainder of this process. Returns whether
    /// this call changed state so callers can emit the unavailable-path warning
    /// exactly once.
    pub fn disable_persistence(&mut self) -> bool {
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
pub struct PerOwnerSaveTimer {
    elapsed: Duration,
    was_connected: bool,
}

impl PerOwnerSaveTimer {
    pub fn observe_connection(&mut self, connected: bool) {
        if connected && !self.was_connected {
            self.elapsed = Duration::ZERO;
        }
        self.was_connected = connected;
    }

    /// Advance only during an active participation generation. Returns `true`
    /// once when the save is due, resetting the cadence for the next interval.
    pub fn advance(&mut self, frame_dt: Duration, participating: bool) -> bool {
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

/// Join `<mod_id>/state.json` onto the data directory it is given (the engine
/// resolves that directory once at boot from the app name). Sim parses no
/// arguments and resolves no platform directory.
pub fn state_path(data_dir: Option<&Path>, mod_id: &str) -> Option<PathBuf> {
    data_dir.map(|data_dir| data_dir.join(mod_id).join(STATE_FILENAME))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PersistedState {
    version: u32,
    slots: BTreeMap<String, PersistedValue>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    per_owner: BTreeMap<String, BTreeMap<String, PersistedValue>>,
    /// Directional live sentiment keyed by authored faction names or reserved
    /// engine identity keys. The nested map preserves pair direction.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    faction_sentiment: BTreeMap<String, BTreeMap<String, f32>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PersistedValue {
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
pub enum PersistenceError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub struct CollectedState {
    pub state: PersistedState,
    pub warnings: Vec<String>,
}

/// Build the save document without performing filesystem I/O. Retained live mod
/// slots absent from the latest committed declarations are not save members.
pub fn collect_persisted_state(
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
            faction_sentiment: BTreeMap::new(),
        },
        warnings,
    }
}

/// Collect the live sparse sentiment overlay into the engine-owned section of
/// a save document. Runtime faction indices are deliberately not durable:
/// authored names and the two reserved engine identities survive reload.
pub fn collect_persisted_faction_sentiment(
    state: &mut PersistedState,
    sentiment: &FactionSentimentState,
    factions: &FactionRegistry,
) -> Vec<String> {
    let mut faction_sentiment = BTreeMap::new();
    let mut warnings = Vec::new();

    for (from, to, value) in sentiment.iter() {
        if !value.is_finite() {
            warnings.push(format!(
                "persistent faction sentiment from index {from} to {to} is non-finite; omitting it"
            ));
            continue;
        }
        let Some(from_name) = faction_name_for_index(factions, from) else {
            warnings.push(format!(
                "persistent faction sentiment from index {from} to {to} has no durable from-faction identity; omitting it"
            ));
            continue;
        };
        let Some(to_name) = faction_name_for_index(factions, to) else {
            warnings.push(format!(
                "persistent faction sentiment from index {from} to {to} has no durable to-faction identity; omitting it"
            ));
            continue;
        };

        faction_sentiment
            .entry(from_name.to_owned())
            .or_insert_with(BTreeMap::new)
            .insert(to_name.to_owned(), value);
    }

    state.faction_sentiment = faction_sentiment;
    warnings
}

/// Overlay name-keyed live sentiment after the immutable faction baseline has
/// committed. Missing names degrade to an omitted runtime pair, just like a
/// malformed persisted slot degrades to its declared default.
pub fn overlay_persisted_faction_sentiment(
    sentiment: &mut FactionSentimentState,
    factions: &FactionRegistry,
    persisted: &PersistedState,
) -> Vec<String> {
    let mut warnings = Vec::new();

    for (from_name, to_values) in &persisted.faction_sentiment {
        let Some(from) = faction_index_for_persisted_name(factions, from_name) else {
            for to_name in to_values.keys() {
                warnings.push(format!(
                    "state file faction sentiment from `{from_name}` to `{to_name}` references an undeclared from faction; ignoring it"
                ));
            }
            continue;
        };

        for (to_name, value) in to_values {
            if !value.is_finite() {
                warnings.push(format!(
                    "state file faction sentiment from `{from_name}` to `{to_name}` is non-finite; ignoring it"
                ));
                continue;
            }
            let Some(to) = faction_index_for_persisted_name(factions, to_name) else {
                warnings.push(format!(
                    "state file faction sentiment from `{from_name}` to `{to_name}` references an undeclared to faction; ignoring it"
                ));
                continue;
            };

            if let Err(error) = sentiment.set(from, to, *value, factions.sentiment(from, to)) {
                warnings.push(format!(
                    "state file faction sentiment from `{from_name}` to `{to_name}` is unsupported: {error}; ignoring it"
                ));
            }
        }
    }

    warnings
}

fn faction_name_for_index(factions: &FactionRegistry, index: usize) -> Option<&str> {
    match index {
        0 => Some(PERSISTED_PLAYER_FACTION_KEY),
        1 => Some(PERSISTED_DEFAULT_ENEMY_FACTION_KEY),
        _ => index
            .checked_sub(2)
            .and_then(|offset| factions.descriptors().get(offset))
            .map(|descriptor| descriptor.name.as_str()),
    }
}

fn faction_index_for_persisted_name(factions: &FactionRegistry, name: &str) -> Option<f32> {
    match name {
        PERSISTED_PLAYER_FACTION_KEY => Some(PLAYER_FACTION_INDEX),
        PERSISTED_DEFAULT_ENEMY_FACTION_KEY => Some(DEFAULT_ENEMY_FACTION_INDEX),
        _ => factions.index_for_name(name),
    }
}

/// Collect one player's persistent per-owner slots. The session-scoped seat is
/// used only to select the live value; the saved key is the durable player id.
pub fn collect_per_owner_state(
    table: &SlotTable,
    identity: Option<&StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
    local_seat: Seat,
    local_player_id: [u8; 16],
) -> CollectedPerOwnerState {
    let encoded_player_id = encode_player_claim_id(&PlayerClaimId(local_player_id));
    let (values, warnings) =
        collect_seat_per_owner_values(table, identity, committed_store_slots, local_seat);
    let per_owner = values
        .into_iter()
        .map(|(durable_key, value)| {
            (
                durable_key,
                BTreeMap::from([(encoded_player_id.clone(), value)]),
            )
        })
        .collect();
    CollectedPerOwnerState {
        per_owner,
        warnings,
    }
}

/// One seat's persisted per-owner values by durable key, as a seat record
/// holds them, with a warning for each slot it had to omit.
pub fn collect_seat_per_owner_values(
    table: &SlotTable,
    identity: Option<&StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
    seat: Seat,
) -> (BTreeMap<String, PersistedValue>, Vec<String>) {
    let mut values = BTreeMap::new();
    let mut warnings = Vec::new();
    for (name, record) in table.iter() {
        if !is_persisted_per_owner_slot(record) || !committed_store_slots.contains(name) {
            continue;
        }

        let Some(value) = record.per_seat_value(seat) else {
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
                values.insert(durable_key.to_owned(), value);
            }
            Err(warning) => warnings.push(warning),
        }
    }
    (values, warnings)
}

pub struct CollectedPerOwnerState {
    pub per_owner: BTreeMap<String, BTreeMap<String, PersistedValue>>,
    pub warnings: Vec<String>,
}

pub fn persisted_state_version_is_supported(persisted: &PersistedState) -> bool {
    (OLDEST_SUPPORTED_STATE_VERSION..=CURRENT_STATE_VERSION).contains(&persisted.version)
}

/// Overlay a decoded save document onto already-declared slots.
///
/// Invalid entries are left at their current declared/default value and
/// returned as warnings for the caller to log. Ledger rows outside current
/// declaration membership cannot target the add-only live table.
pub fn overlay_persisted_state(
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

    warnings.extend(overlay_per_owner_state(
        table,
        persisted,
        &authored_by_key,
        local_player_id,
        local_seat,
        |_, _| true,
    ));
    warnings
}

fn authored_names_by_durable_key<'a>(
    identity: Option<&'a StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
) -> BTreeMap<&'a str, &'a str> {
    identity
        .map(|ledger| {
            ledger
                .slots
                .iter()
                .filter(|(name, _)| committed_store_slots.contains(name.as_str()))
                .map(|(name, key)| (key.as_str(), name.as_str()))
                .collect()
        })
        .unwrap_or_default()
}

fn overlay_per_owner_state(
    table: &mut SlotTable,
    persisted: &PersistedState,
    authored_by_key: &BTreeMap<&str, &str>,
    local_player_id: Option<[u8; 16]>,
    local_seat: Seat,
    applies_to: impl Fn(&str, &SlotRecord) -> bool,
) -> Vec<String> {
    let Some(local_player_id) = local_player_id else {
        return Vec::new();
    };
    let local_player_id = PlayerClaimId(local_player_id);
    let mut warnings = Vec::new();
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
        if !applies_to(name, record) {
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

fn unsupported_state_version_warning(version: u32) -> String {
    format!(
        "state file version {version} is not supported (current version is {CURRENT_STATE_VERSION}); ignoring file"
    )
}

pub fn load_persisted_state(path: &Path) -> Result<Option<PersistedState>, PersistenceError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok(Some(serde_json::from_slice(&bytes)?))
}

pub fn save_persisted_state(path: &Path, state: &PersistedState) -> Result<(), PersistenceError> {
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
pub fn encode_player_claim_id(player_id: &PlayerClaimId) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(player_id.0.len() * 2);
    for byte in player_id.0 {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// Decode a 32-character hexadecimal player-claim map key.
pub fn decode_player_claim_id(encoded: &str) -> Option<PlayerClaimId> {
    if encoded.len() != 32 {
        return None;
    }

    let mut player_id = [0; 16];
    for (index, pair) in encoded.as_bytes().as_chunks::<2>().0.iter().enumerate() {
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
