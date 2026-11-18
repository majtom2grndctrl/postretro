// The durable ledger document: one serialization for character, world and run saves.
// See: context/lib/networking.md §Session-state ledger

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::loadout::LoadoutRecord;
use super::{CURRENT_SAVE_VERSION, OLDEST_SUPPORTED_SAVE_VERSION, SaveFileError};
use crate::scripting::state_persistence::PersistedValue;

const VERSION: &str = "version";
const LEVEL: &str = "level";
const RUN: &str = "run";
const SLOTS: &str = "slots";
const FACTION_SENTIMENT: &str = "factionSentiment";
const SEATS: &str = "seats";

/// The durable part of the session-state ledger. A character file is a
/// document holding one seat record; a world file holds global slots; a run
/// file holds a level, run-scoped global slots and faction sentiment.
/// Connections, holds and placement are live state and have no field here.
///
/// Each top-level key is a named section. A section this build does not know
/// is kept as its raw bytes and written back unchanged, so a later section
/// (revisited-map state, a host-migration handoff) needs no version bump.
#[derive(Clone, Debug, Default)]
pub struct LedgerDocument {
    pub level: Option<LevelRecord>,
    pub run: Option<RunRecord>,
    /// Global slot values by durable key.
    pub slots: BTreeMap<String, PersistedValue>,
    /// Directional faction sentiment, as `state.json` stores it.
    pub faction_sentiment: BTreeMap<String, BTreeMap<String, f32>>,
    pub seats: Vec<SeatRecord>,
    /// Sections this build does not know, kept as their raw bytes.
    pub unknown: BTreeMap<String, Box<RawValue>>,
}

/// The level a run last reached, and the checkpoint within it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelRecord {
    pub catalog_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
}

/// What makes a run file a run: the character it belongs to and when it was
/// last written.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    /// The owning character's claim, as 32 hex characters.
    pub character: String,
    /// Wall-clock UTC seconds since the Unix epoch.
    pub saved_at: u64,
}

/// One seat's durable record: who it is and what it carries.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatRecord {
    /// The claim the character joins with, as 32 hex characters.
    pub claim: String,
    #[serde(default)]
    pub name: String,
    /// Per-owner slot values by durable key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub per_owner: BTreeMap<String, PersistedValue>,
    #[serde(default)]
    pub loadout: LoadoutRecord,
}

impl PartialEq for LedgerDocument {
    fn eq(&self, other: &Self) -> bool {
        self.level == other.level
            && self.run == other.run
            && self.slots == other.slots
            && self.faction_sentiment == other.faction_sentiment
            && self.seats == other.seats
            && self.unknown.len() == other.unknown.len()
            && self
                .unknown
                .iter()
                .zip(&other.unknown)
                .all(|((a_name, a), (b_name, b))| a_name == b_name && a.get() == b.get())
    }
}

impl LedgerDocument {
    /// Parse a document. The version is checked before any other section is
    /// read, so a document from an unsupported build is never half-parsed.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, SaveFileError> {
        let mut sections: BTreeMap<String, Box<RawValue>> = serde_json::from_slice(bytes)?;
        let version: u32 = section(&mut sections, VERSION)?.ok_or_else(|| {
            SaveFileError::MalformedSection {
                section: VERSION,
                error: serde::de::Error::missing_field(VERSION),
            }
        })?;
        if !(OLDEST_SUPPORTED_SAVE_VERSION..=CURRENT_SAVE_VERSION).contains(&version) {
            return Err(SaveFileError::UnsupportedVersion(version));
        }
        Ok(Self {
            level: section(&mut sections, LEVEL)?,
            run: section(&mut sections, RUN)?,
            slots: section(&mut sections, SLOTS)?.unwrap_or_default(),
            faction_sentiment: section(&mut sections, FACTION_SENTIMENT)?.unwrap_or_default(),
            seats: section(&mut sections, SEATS)?.unwrap_or_default(),
            unknown: sections,
        })
    }

    /// Serialize at the current version, writing every unknown section back
    /// byte-for-byte.
    pub fn to_vec(&self) -> Result<Vec<u8>, SaveFileError> {
        let mut sections = self.unknown.clone();
        sections.insert(VERSION.into(), raw(&CURRENT_SAVE_VERSION)?);
        if let Some(level) = &self.level {
            sections.insert(LEVEL.into(), raw(level)?);
        }
        if let Some(run) = &self.run {
            sections.insert(RUN.into(), raw(run)?);
        }
        if !self.slots.is_empty() {
            sections.insert(SLOTS.into(), raw(&self.slots)?);
        }
        if !self.faction_sentiment.is_empty() {
            sections.insert(FACTION_SENTIMENT.into(), raw(&self.faction_sentiment)?);
        }
        if !self.seats.is_empty() {
            sections.insert(SEATS.into(), raw(&self.seats)?);
        }
        Ok(serde_json::to_vec_pretty(&sections)?)
    }

    /// The names of the sections this build did not recognize.
    pub fn unknown_sections(&self) -> impl Iterator<Item = &str> {
        self.unknown.keys().map(String::as_str)
    }
}

fn section<T: DeserializeOwned>(
    sections: &mut BTreeMap<String, Box<RawValue>>,
    name: &'static str,
) -> Result<Option<T>, SaveFileError> {
    sections
        .remove(name)
        .map(|raw| {
            serde_json::from_str(raw.get())
                .map_err(|error| SaveFileError::MalformedSection { section: name, error })
        })
        .transpose()
}

fn raw<T: Serialize + ?Sized>(value: &T) -> Result<Box<RawValue>, SaveFileError> {
    Ok(serde_json::value::to_raw_value(value)?)
}
