// State-slot wire types: schema-driven replication of authoritative store slots.
// See: context/lib/networking.md
//
// This module owns the wire-facing half of M15 Phase 3.5 state replication. It is
// deliberately split out of `wire.rs` (already past the split-before-extend
// threshold), which keeps only the snapshot/ack/control envelope glue. The state
// records ride *inside* `RawSnapshotMessage` on the existing unreliable snapshot
// channel; acks ride in `AckMessage` and refresh requests in `ClientMessage` on
// the reliable input channel.
//
// Registry-blind and script-blind by construction. `postretro-net` never sees a
// `SlotTable`, `SlotValue`, scripting types, descriptor types, or `glam`. A slot is
// identified on the wire by a deterministic `StateSlotId(u16)` plus a 32-byte
// opaque schema fingerprint; the engine (`postretro`) owns the
// `StateSlotId -> dotted name` map, computes the fingerprint with `blake3`, and
// lowers each replicated slot to a `StateSlotDescriptor` for validation here. The
// fingerprint is opaque bytes to this crate: it stores and compares it, never
// recomputes it, so it has no `blake3` dependency.
//
// As with the entity record path in `wire.rs`, the *raw* record carries explicit
// numeric `kind` discriminants and `Option` slots so a malformed envelope decodes
// cleanly and is rejected at `validate` time — never at decode, and never by
// reaching the slot table.

use std::collections::HashMap;

use bitcode::{Decode, Encode};

/// Deterministic wire identity for one replicated state slot. Assigned by the
/// engine from the sorted replicated-slot schema; the dotted slot name never
/// crosses the wire in a per-tick record. The inner `u16` is encoded transparently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Encode, Decode)]
pub struct StateSlotId(pub u16);

/// Maximum number of state records permitted in one snapshot. A snapshot carrying
/// more is rejected before any record is applied, so a hostile peer cannot force
/// unbounded per-frame work.
pub const MAX_STATE_RECORDS_PER_SNAPSHOT: usize = 1024;

/// Maximum byte length of a wire string / enum value (UTF-8 bytes, not chars).
pub const MAX_STRING_BYTES: usize = 256;

/// Maximum element count of a wire numeric array value.
pub const MAX_ARRAY_ELEMENTS: usize = 64;

/// `kind` discriminant for a full-baseline state record (join / refresh / value
/// first sight). Mirrors the entity-record full-baseline discriminant.
pub const STATE_RECORD_KIND_FULL_BASELINE: u16 = 0;
/// `kind` discriminant for a delta state record (a new complete value carried
/// against a referenced baseline).
pub const STATE_RECORD_KIND_DELTA: u16 = 1;

/// `kind` discriminant for a full faction-sentiment baseline. Unlike a state-slot
/// record, this carries the complete sparse overlay rather than one descriptor-backed
/// scalar value.
pub const FACTION_SENTIMENT_RECORD_KIND_FULL_BASELINE: u16 = 0;
/// `kind` discriminant for a faction-sentiment update against the client's held
/// baseline. The payload is still the complete sparse set so removals are explicit
/// by absence and the engine can replace its overlay atomically.
pub const FACTION_SENTIMENT_RECORD_KIND_DELTA: u16 = 1;

/// Maximum sparse faction-sentiment pairs accepted in one snapshot record. The
/// overlay is intentionally sparse; this bounds hostile decode work before the
/// engine touches its live state.
pub const MAX_FACTION_SENTIMENT_PAIRS: usize = 4096;

/// `value_kind` discriminant for the explicit "no current value" state.
pub const VALUE_KIND_UNSET: u16 = 0;
/// `value_kind` discriminant for a finite `f32` number value.
pub const VALUE_KIND_NUMBER: u16 = 1;
/// `value_kind` discriminant for a boolean value.
pub const VALUE_KIND_BOOLEAN: u16 = 2;
/// `value_kind` discriminant for a UTF-8 string value (≤ 256 bytes).
pub const VALUE_KIND_STRING: u16 = 3;
/// `value_kind` discriminant for a UTF-8 enum value (≤ 256 bytes, schema-checked).
pub const VALUE_KIND_ENUM: u16 = 4;
/// `value_kind` discriminant for a finite `f32` array value (≤ 64 elements).
pub const VALUE_KIND_ARRAY: u16 = 5;

// ---------------------------------------------------------------------------
// Replication scope and slot value type
// ---------------------------------------------------------------------------

/// Wire-safe replication scope tag. The engine maps its scripting-aware scope enum
/// onto this; this crate uses it only to fold scope into the descriptor (the engine
/// already folded scope into the fingerprint). `None` slots are never lowered to a
/// descriptor — they receive no `StateSlotId` — so only the two replicated scopes
/// appear here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub enum ReplicationScope {
    /// Server sends this slot to every accepted client.
    SharedGlobal,
    /// Server sends this slot only to the owning accepted client.
    OwnerPrivatePlayer,
}

/// Wire-safe value-type tag for a replicated slot, used by the descriptor to type-
/// check incoming `WireSlotValue`s. Enum carries its declared values in order so a
/// hostile enum value is rejected here without the engine round-trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotValueType {
    Number,
    Boolean,
    String,
    Enum { values: Vec<String> },
    Array,
}

/// Inclusive numeric bounds lowered from the engine's slot range. `min`/`max` are
/// only meaningful when the corresponding `*_finite` flag is set; an unbounded edge
/// (e.g. `+inf` max) lowers as `finite = false`, so this crate never compares
/// against a non-finite bound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumericRange {
    pub min: f32,
    pub max: f32,
    pub min_finite: bool,
    pub max_finite: bool,
}

/// Wire-safe descriptor for one replicated slot, lowered by the engine. This crate
/// validates hostile bytes against it. It is *not* a wire type itself — it lives on
/// both peers as the local schema, keyed by `StateSlotId`. The 32-byte fingerprint
/// is opaque: the descriptor set carries it for the snapshot-level fingerprint gate.
#[derive(Debug, Clone, PartialEq)]
pub struct StateSlotDescriptor {
    pub slot_id: StateSlotId,
    pub value_type: SlotValueType,
    pub range: Option<NumericRange>,
    pub scope: ReplicationScope,
}

/// The engine-lowered local schema: every replicated slot descriptor plus the
/// opaque fingerprint the engine computed over the canonical schema byte stream.
/// Both peers build this from their own slot tables; a snapshot whose
/// `state_schema_fingerprint` does not match `fingerprint` here is rejected before
/// any record is applied.
#[derive(Debug, Clone, PartialEq)]
pub struct StateSchema {
    fingerprint: [u8; 32],
    descriptors: HashMap<StateSlotId, StateSlotDescriptor>,
}

// ---------------------------------------------------------------------------
// Faction-sentiment sparse sync record
// ---------------------------------------------------------------------------

/// One directional diverged faction-sentiment value. Faction indices are content
/// identities, so this deliberately carries compact indices instead of names. The
/// engine validates that both indices exist in its immutable faction registry before
/// replacing the live overlay.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct FactionSentimentPair {
    pub from_idx: u16,
    pub to_idx: u16,
    pub value: f32,
}

/// Raw faction-sentiment overlay record carried in [`crate::wire::RawSnapshotMessage`].
/// It has its own baseline namespace and is intentionally independent of
/// [`StateSlotDescriptor`]: a mutable, variable-length sparse set cannot be modeled as
/// a static declared slot.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct RawFactionSentimentRecord {
    pub kind: u16,
    /// `FullBaseline` requires `false`; `Delta` requires `true`.
    pub has_baseline_ref: bool,
    pub baseline_ref: u32,
    /// The baseline established by this complete sparse-set snapshot.
    pub baseline_id: u32,
    /// All diverged pairs, in strictly ascending `(from_idx, to_idx)` order.
    pub pairs: Vec<FactionSentimentPair>,
}

/// Validated faction-sentiment sync record. The net crate validates only wire-local
/// facts (record kind, baseline shape, pair ordering, cap, and finite values); the
/// engine later validates that indices exist in the installed faction registry.
#[derive(Debug, Clone, PartialEq)]
pub enum FactionSentimentRecord {
    FullBaseline {
        baseline_id: u32,
        pairs: Vec<FactionSentimentPair>,
    },
    Delta {
        baseline_ref: u32,
        new_baseline_id: u32,
        pairs: Vec<FactionSentimentPair>,
    },
}

impl FactionSentimentRecord {
    /// The complete sparse set carried by this record. Deltas are replacement sets,
    /// not per-pair arithmetic changes, so an absent pair clears a former override.
    #[must_use]
    pub fn pairs(&self) -> &[FactionSentimentPair] {
        match self {
            Self::FullBaseline { pairs, .. } | Self::Delta { pairs, .. } => pairs,
        }
    }

    #[must_use]
    pub fn baseline_id(&self) -> u32 {
        match self {
            Self::FullBaseline { baseline_id, .. } => *baseline_id,
            Self::Delta {
                new_baseline_id, ..
            } => *new_baseline_id,
        }
    }
}

/// Why a structural faction-sentiment record must not reach the engine overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactionSentimentValidationError {
    TooManyPairs { count: usize },
    UnknownRecordKind(u16),
    BadBaselineCombination { kind: u16, has_ref: bool },
    ZeroBaselineId { kind: u16 },
    ZeroBaselineRef { kind: u16 },
    NonFiniteValue { from_idx: u16, to_idx: u16 },
    PairsNotStrictlySorted,
}

impl std::fmt::Display for FactionSentimentValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyPairs { count } => write!(
                f,
                "faction-sentiment record carried {count} pairs (max {MAX_FACTION_SENTIMENT_PAIRS})"
            ),
            Self::UnknownRecordKind(kind) => {
                write!(f, "unknown faction-sentiment record kind {kind}")
            }
            Self::BadBaselineCombination { kind, has_ref } => write!(
                f,
                "illegal baseline combination for faction-sentiment record kind {kind} (has_ref={has_ref})"
            ),
            Self::ZeroBaselineId { kind } => {
                write!(
                    f,
                    "faction-sentiment record kind {kind} used reserved baseline id 0"
                )
            }
            Self::ZeroBaselineRef { kind } => write!(
                f,
                "faction-sentiment delta record kind {kind} used reserved baseline ref 0"
            ),
            Self::NonFiniteValue { from_idx, to_idx } => write!(
                f,
                "non-finite faction-sentiment value for pair ({from_idx}, {to_idx})"
            ),
            Self::PairsNotStrictlySorted => {
                write!(f, "faction-sentiment pairs were not strictly sorted")
            }
        }
    }
}

impl std::error::Error for FactionSentimentValidationError {}

impl RawFactionSentimentRecord {
    /// Validate a raw sparse-set record before the engine resolves its faction
    /// indices. One malformed pair rejects the whole record, preserving the client's
    /// prior overlay exactly.
    pub fn validate(&self) -> Result<FactionSentimentRecord, FactionSentimentValidationError> {
        if self.pairs.len() > MAX_FACTION_SENTIMENT_PAIRS {
            return Err(FactionSentimentValidationError::TooManyPairs {
                count: self.pairs.len(),
            });
        }
        if !self.has_baseline_ref && self.baseline_ref != 0 {
            return Err(FactionSentimentValidationError::BadBaselineCombination {
                kind: self.kind,
                has_ref: self.has_baseline_ref,
            });
        }
        if self.baseline_id == 0 {
            return Err(FactionSentimentValidationError::ZeroBaselineId { kind: self.kind });
        }
        if self.has_baseline_ref && self.baseline_ref == 0 {
            return Err(FactionSentimentValidationError::ZeroBaselineRef { kind: self.kind });
        }
        let mut previous = None;
        for pair in &self.pairs {
            if !pair.value.is_finite() {
                return Err(FactionSentimentValidationError::NonFiniteValue {
                    from_idx: pair.from_idx,
                    to_idx: pair.to_idx,
                });
            }
            let key = (pair.from_idx, pair.to_idx);
            if previous.is_some_and(|previous| previous >= key) {
                return Err(FactionSentimentValidationError::PairsNotStrictlySorted);
            }
            previous = Some(key);
        }

        match self.kind {
            FACTION_SENTIMENT_RECORD_KIND_FULL_BASELINE if !self.has_baseline_ref => {
                Ok(FactionSentimentRecord::FullBaseline {
                    baseline_id: self.baseline_id,
                    pairs: self.pairs.clone(),
                })
            }
            FACTION_SENTIMENT_RECORD_KIND_DELTA if self.has_baseline_ref => {
                Ok(FactionSentimentRecord::Delta {
                    baseline_ref: self.baseline_ref,
                    new_baseline_id: self.baseline_id,
                    pairs: self.pairs.clone(),
                })
            }
            FACTION_SENTIMENT_RECORD_KIND_FULL_BASELINE | FACTION_SENTIMENT_RECORD_KIND_DELTA => {
                Err(FactionSentimentValidationError::BadBaselineCombination {
                    kind: self.kind,
                    has_ref: self.has_baseline_ref,
                })
            }
            kind => Err(FactionSentimentValidationError::UnknownRecordKind(kind)),
        }
    }
}

impl StateSchema {
    /// Build a schema from the engine's fingerprint and lowered descriptors. Later
    /// descriptors with a duplicate `slot_id` overwrite earlier ones; the engine
    /// assigns ids deterministically and never duplicates, so this is a defensive
    /// last-wins, not an expected path.
    #[must_use]
    pub fn new(
        fingerprint: [u8; 32],
        descriptors: impl IntoIterator<Item = StateSlotDescriptor>,
    ) -> Self {
        let descriptors = descriptors
            .into_iter()
            .map(|d| (d.slot_id, d))
            .collect::<HashMap<_, _>>();
        Self {
            fingerprint,
            descriptors,
        }
    }

    #[must_use]
    pub fn fingerprint(&self) -> &[u8; 32] {
        &self.fingerprint
    }

    #[must_use]
    pub fn descriptor(&self, slot_id: StateSlotId) -> Option<&StateSlotDescriptor> {
        self.descriptors.get(&slot_id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Wire value
// ---------------------------------------------------------------------------

/// A replicated slot value on the wire. Mirrors the engine `Option<SlotValue>`
/// model: `Unset` is the explicit "no current value" state (the engine's `None`),
/// and the variants mirror `SlotValue`. Caps and finiteness are *not* enforced by
/// construction — a hostile peer can encode an over-cap string or a NaN — so they
/// are checked at [`WireSlotValue::validate_against`] before any apply.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum WireSlotValue {
    Unset,
    Number(f32),
    Boolean(bool),
    String(String),
    Enum(String),
    Array(Vec<f32>),
}

/// Why a structurally-decodable state record/value is not valid against the local
/// schema. These are *semantic* rejections after a clean bitcode decode — a corrupt
/// or truncated buffer is a `wire::WireError` at decode, never one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateValidationError {
    /// The snapshot's `state_schema_fingerprint` did not match the local schema.
    SchemaFingerprintMismatch,
    /// A snapshot carried more than [`MAX_STATE_RECORDS_PER_SNAPSHOT`] state records.
    TooManyStateRecords { count: usize },
    /// `kind` was not one of the defined state-record discriminants.
    UnknownRecordKind(u16),
    /// `value_kind` was not one of the defined value discriminants.
    UnknownValueKind(u16),
    /// A `FullBaseline` carried a `baseline_ref`, or a `Delta` carried none.
    BadBaselineCombination { kind: u16, has_ref: bool },
    /// No local descriptor exists for the record's `slot_id`.
    UnknownSlotId(u16),
    /// The value kind does not match the slot's declared type.
    TypeMismatch { slot_id: u16 },
    /// A number or array element was non-finite (NaN/inf).
    NonFiniteValue { slot_id: u16 },
    /// A string/enum value exceeded [`MAX_STRING_BYTES`].
    StringTooLong { slot_id: u16, bytes: usize },
    /// An array value exceeded [`MAX_ARRAY_ELEMENTS`].
    ArrayTooLong { slot_id: u16, elements: usize },
    /// An enum value was not one of the slot's declared enum values.
    UnknownEnumValue { slot_id: u16 },
}

impl std::fmt::Display for StateValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateValidationError::SchemaFingerprintMismatch => {
                write!(f, "state schema fingerprint mismatch")
            }
            StateValidationError::TooManyStateRecords { count } => write!(
                f,
                "snapshot carried {count} state records (max {MAX_STATE_RECORDS_PER_SNAPSHOT})"
            ),
            StateValidationError::UnknownRecordKind(k) => {
                write!(f, "unknown state record kind {k}")
            }
            StateValidationError::UnknownValueKind(k) => write!(f, "unknown slot value kind {k}"),
            StateValidationError::BadBaselineCombination { kind, has_ref } => write!(
                f,
                "illegal baseline combination for state record kind {kind} (has_ref={has_ref})"
            ),
            StateValidationError::UnknownSlotId(id) => write!(f, "unknown state slot id {id}"),
            StateValidationError::TypeMismatch { slot_id } => {
                write!(f, "value type mismatch for state slot id {slot_id}")
            }
            StateValidationError::NonFiniteValue { slot_id } => {
                write!(f, "non-finite value for state slot id {slot_id}")
            }
            StateValidationError::StringTooLong { slot_id, bytes } => write!(
                f,
                "string value for state slot id {slot_id} is {bytes} bytes (max {MAX_STRING_BYTES})"
            ),
            StateValidationError::ArrayTooLong { slot_id, elements } => write!(
                f,
                "array value for state slot id {slot_id} has {elements} elements (max {MAX_ARRAY_ELEMENTS})"
            ),
            StateValidationError::UnknownEnumValue { slot_id } => {
                write!(
                    f,
                    "enum value for state slot id {slot_id} is not a declared value"
                )
            }
        }
    }
}

impl std::error::Error for StateValidationError {}

impl WireSlotValue {
    /// The wire discriminant for this value, mirroring `VALUE_KIND_*`. Used by the
    /// raw <-> typed conversion and pinned by tests against the constants.
    #[must_use]
    pub fn kind(&self) -> u16 {
        match self {
            WireSlotValue::Unset => VALUE_KIND_UNSET,
            WireSlotValue::Number(_) => VALUE_KIND_NUMBER,
            WireSlotValue::Boolean(_) => VALUE_KIND_BOOLEAN,
            WireSlotValue::String(_) => VALUE_KIND_STRING,
            WireSlotValue::Enum(_) => VALUE_KIND_ENUM,
            WireSlotValue::Array(_) => VALUE_KIND_ARRAY,
        }
    }

    /// Validate this value against a slot descriptor: type match, finiteness, caps,
    /// and enum membership. `Unset` is always valid (it clears the slot regardless
    /// of type). Returns `Ok(())` only when the value is safe to apply.
    pub fn validate_against(
        &self,
        descriptor: &StateSlotDescriptor,
    ) -> Result<(), StateValidationError> {
        let slot_id = descriptor.slot_id.0;
        match self {
            WireSlotValue::Unset => Ok(()),
            WireSlotValue::Number(value) => {
                if !matches!(descriptor.value_type, SlotValueType::Number) {
                    return Err(StateValidationError::TypeMismatch { slot_id });
                }
                if !value.is_finite() {
                    return Err(StateValidationError::NonFiniteValue { slot_id });
                }
                Ok(())
            }
            WireSlotValue::Boolean(_) => {
                if matches!(descriptor.value_type, SlotValueType::Boolean) {
                    Ok(())
                } else {
                    Err(StateValidationError::TypeMismatch { slot_id })
                }
            }
            WireSlotValue::String(value) => {
                if !matches!(descriptor.value_type, SlotValueType::String) {
                    return Err(StateValidationError::TypeMismatch { slot_id });
                }
                check_string_cap(slot_id, value)
            }
            WireSlotValue::Enum(value) => {
                let SlotValueType::Enum { values } = &descriptor.value_type else {
                    return Err(StateValidationError::TypeMismatch { slot_id });
                };
                check_string_cap(slot_id, value)?;
                if values.iter().any(|declared| declared == value) {
                    Ok(())
                } else {
                    Err(StateValidationError::UnknownEnumValue { slot_id })
                }
            }
            WireSlotValue::Array(elements) => {
                if !matches!(descriptor.value_type, SlotValueType::Array) {
                    return Err(StateValidationError::TypeMismatch { slot_id });
                }
                if elements.len() > MAX_ARRAY_ELEMENTS {
                    return Err(StateValidationError::ArrayTooLong {
                        slot_id,
                        elements: elements.len(),
                    });
                }
                if elements.iter().all(|e| e.is_finite()) {
                    Ok(())
                } else {
                    Err(StateValidationError::NonFiniteValue { slot_id })
                }
            }
        }
    }
}

fn check_string_cap(slot_id: u16, value: &str) -> Result<(), StateValidationError> {
    if value.len() > MAX_STRING_BYTES {
        Err(StateValidationError::StringTooLong {
            slot_id,
            bytes: value.len(),
        })
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Raw encoded boundary
// ---------------------------------------------------------------------------

/// Raw per-slot state record as it crosses the wire. `kind` selects full-baseline
/// vs delta; `has_baseline_ref`/`baseline_ref` mirror the typed `Option<u32>`
/// (a `false` flag with a nonzero ref is a malformed envelope, rejected at
/// `validate`). `baseline_id` is always the new baseline id for this value.
///
/// As with `RawEntityRecord`, the explicit `kind`/`value` shape makes a malformed
/// record *representable* in the decoded envelope so it is a testable `validate`
/// rejection, not a decode error.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct RawStateSlotRecord {
    pub slot_id: u16,
    pub kind: u16,
    /// Whether `baseline_ref` carries a real value. `FullBaseline` requires `false`;
    /// `Delta` requires `true`.
    pub has_baseline_ref: bool,
    pub baseline_ref: u32,
    /// The new baseline id this value establishes (full baseline) or advances to
    /// (delta). Always meaningful.
    pub baseline_id: u32,
    pub value: WireSlotValue,
}

/// A validated state record: the typed apply model. Produced only by
/// [`RawStateSlotRecord::validate`], so a typed record always has a legal
/// baseline combination and a value that type-checks against its slot.
#[derive(Debug, Clone, PartialEq)]
pub enum StateSlotRecord {
    FullBaseline {
        slot_id: StateSlotId,
        baseline_id: u32,
        value: WireSlotValue,
    },
    Delta {
        slot_id: StateSlotId,
        baseline_ref: u32,
        new_baseline_id: u32,
        value: WireSlotValue,
    },
}

impl StateSlotRecord {
    #[must_use]
    pub fn slot_id(&self) -> StateSlotId {
        match self {
            StateSlotRecord::FullBaseline { slot_id, .. }
            | StateSlotRecord::Delta { slot_id, .. } => *slot_id,
        }
    }

    /// The complete value this record carries (a state delta is a full value, not a
    /// numeric diff).
    #[must_use]
    pub fn value(&self) -> &WireSlotValue {
        match self {
            StateSlotRecord::FullBaseline { value, .. } | StateSlotRecord::Delta { value, .. } => {
                value
            }
        }
    }
}

impl RawStateSlotRecord {
    /// Validate this raw record against the local schema into a typed
    /// [`StateSlotRecord`]. Rejects unknown kinds, illegal baseline combinations,
    /// unknown slot ids, and any value that fails its slot's validation. The value's
    /// own caps/finiteness/type are checked here too, so a typed record is always
    /// safe to apply.
    pub fn validate(&self, schema: &StateSchema) -> Result<StateSlotRecord, StateValidationError> {
        // Reject an unknown value kind before touching the slot: a hostile
        // discriminant must not be silently mapped to a known variant.
        let value_kind = self.value.kind();
        if !is_known_value_kind(value_kind) {
            return Err(StateValidationError::UnknownValueKind(value_kind));
        }

        let descriptor = schema
            .descriptor(StateSlotId(self.slot_id))
            .ok_or(StateValidationError::UnknownSlotId(self.slot_id))?;

        // Baseline-ref flag must be internally consistent: an "absent" flag cannot
        // ride a real ref value.
        if !self.has_baseline_ref && self.baseline_ref != 0 {
            return Err(StateValidationError::BadBaselineCombination {
                kind: self.kind,
                has_ref: self.has_baseline_ref,
            });
        }

        match self.kind {
            STATE_RECORD_KIND_FULL_BASELINE => {
                if self.has_baseline_ref {
                    return Err(StateValidationError::BadBaselineCombination {
                        kind: self.kind,
                        has_ref: true,
                    });
                }
                self.value.validate_against(descriptor)?;
                Ok(StateSlotRecord::FullBaseline {
                    slot_id: descriptor.slot_id,
                    baseline_id: self.baseline_id,
                    value: self.value.clone(),
                })
            }
            STATE_RECORD_KIND_DELTA => {
                if !self.has_baseline_ref {
                    return Err(StateValidationError::BadBaselineCombination {
                        kind: self.kind,
                        has_ref: false,
                    });
                }
                self.value.validate_against(descriptor)?;
                Ok(StateSlotRecord::Delta {
                    slot_id: descriptor.slot_id,
                    baseline_ref: self.baseline_ref,
                    new_baseline_id: self.baseline_id,
                    value: self.value.clone(),
                })
            }
            other => Err(StateValidationError::UnknownRecordKind(other)),
        }
    }
}

fn is_known_value_kind(kind: u16) -> bool {
    matches!(
        kind,
        VALUE_KIND_UNSET
            | VALUE_KIND_NUMBER
            | VALUE_KIND_BOOLEAN
            | VALUE_KIND_STRING
            | VALUE_KIND_ENUM
            | VALUE_KIND_ARRAY
    )
}

/// Validate a snapshot's raw state records against the local schema. Checks the
/// snapshot-level fingerprint first, then the record-count cap, then every record.
/// The first rejection short-circuits — no partial typed batch is produced, and
/// nothing reaches the slot table.
///
/// Validating the whole batch up front is the contract the engine apply path
/// relies on: any invalid record rejects the entire batch and leaves prior slot
/// values unchanged.
pub fn validate_state_records(
    schema: &StateSchema,
    snapshot_fingerprint: &[u8; 32],
    records: &[RawStateSlotRecord],
) -> Result<Vec<StateSlotRecord>, StateValidationError> {
    if snapshot_fingerprint != schema.fingerprint() {
        return Err(StateValidationError::SchemaFingerprintMismatch);
    }
    if records.len() > MAX_STATE_RECORDS_PER_SNAPSHOT {
        return Err(StateValidationError::TooManyStateRecords {
            count: records.len(),
        });
    }
    records.iter().map(|r| r.validate(schema)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{decode, encode};

    fn number_descriptor(slot_id: u16) -> StateSlotDescriptor {
        StateSlotDescriptor {
            slot_id: StateSlotId(slot_id),
            value_type: SlotValueType::Number,
            range: Some(NumericRange {
                min: 0.0,
                max: 100.0,
                min_finite: true,
                max_finite: true,
            }),
            scope: ReplicationScope::OwnerPrivatePlayer,
        }
    }

    fn enum_descriptor(slot_id: u16) -> StateSlotDescriptor {
        StateSlotDescriptor {
            slot_id: StateSlotId(slot_id),
            value_type: SlotValueType::Enum {
                values: vec!["pointer".to_string(), "focus".to_string()],
            },
            range: None,
            scope: ReplicationScope::SharedGlobal,
        }
    }

    fn array_descriptor(slot_id: u16) -> StateSlotDescriptor {
        StateSlotDescriptor {
            slot_id: StateSlotId(slot_id),
            value_type: SlotValueType::Array,
            range: None,
            scope: ReplicationScope::SharedGlobal,
        }
    }

    fn schema_with(descriptors: Vec<StateSlotDescriptor>) -> StateSchema {
        StateSchema::new([7u8; 32], descriptors)
    }

    fn full_baseline(slot_id: u16, baseline_id: u32, value: WireSlotValue) -> RawStateSlotRecord {
        RawStateSlotRecord {
            slot_id,
            kind: STATE_RECORD_KIND_FULL_BASELINE,
            has_baseline_ref: false,
            baseline_ref: 0,
            baseline_id,
            value,
        }
    }

    fn delta(
        slot_id: u16,
        baseline_ref: u32,
        baseline_id: u32,
        value: WireSlotValue,
    ) -> RawStateSlotRecord {
        RawStateSlotRecord {
            slot_id,
            kind: STATE_RECORD_KIND_DELTA,
            has_baseline_ref: true,
            baseline_ref,
            baseline_id,
            value,
        }
    }

    // --- Round-trip: every value kind plus unset ---

    #[test]
    fn wire_slot_value_round_trips_every_kind() {
        let values = [
            WireSlotValue::Unset,
            WireSlotValue::Number(42.5),
            WireSlotValue::Boolean(true),
            WireSlotValue::String("hello".to_string()),
            WireSlotValue::Enum("focus".to_string()),
            WireSlotValue::Array(vec![1.0, -2.0, 3.5]),
        ];
        for value in values {
            let bytes = encode(&value);
            let decoded: WireSlotValue = decode(&bytes).expect("value decodes");
            assert_eq!(decoded, value);
        }
    }

    #[test]
    fn raw_state_record_round_trips() {
        for record in [
            full_baseline(3, 9, WireSlotValue::Number(50.0)),
            delta(3, 9, 10, WireSlotValue::Number(60.0)),
            full_baseline(5, 1, WireSlotValue::Unset),
        ] {
            let bytes = encode(&record);
            let decoded: RawStateSlotRecord = decode(&bytes).expect("record decodes");
            assert_eq!(decoded, record);
        }
    }

    fn faction_full_baseline(
        baseline_id: u32,
        pairs: Vec<FactionSentimentPair>,
    ) -> RawFactionSentimentRecord {
        RawFactionSentimentRecord {
            kind: FACTION_SENTIMENT_RECORD_KIND_FULL_BASELINE,
            has_baseline_ref: false,
            baseline_ref: 0,
            baseline_id,
            pairs,
        }
    }

    #[test]
    fn faction_sentiment_sparse_record_round_trips_and_validates() {
        let raw = faction_full_baseline(
            7,
            vec![
                FactionSentimentPair {
                    from_idx: 2,
                    to_idx: 3,
                    value: -0.75,
                },
                FactionSentimentPair {
                    from_idx: 3,
                    to_idx: 2,
                    value: 0.25,
                },
            ],
        );
        let bytes = encode(&raw);
        let decoded: RawFactionSentimentRecord = decode(&bytes).expect("record decodes");
        assert_eq!(decoded, raw);
        assert_eq!(
            decoded.validate(),
            Ok(FactionSentimentRecord::FullBaseline {
                baseline_id: 7,
                pairs: raw.pairs,
            })
        );
    }

    #[test]
    fn malformed_faction_sentiment_record_rejects_whole_sparse_set() {
        let mut raw = faction_full_baseline(
            7,
            vec![FactionSentimentPair {
                from_idx: 3,
                to_idx: 2,
                value: -0.75,
            }],
        );
        raw.pairs.push(FactionSentimentPair {
            from_idx: 2,
            to_idx: 3,
            value: 0.25,
        });
        assert_eq!(
            raw.validate(),
            Err(FactionSentimentValidationError::PairsNotStrictlySorted)
        );

        raw.pairs.sort_by_key(|pair| (pair.from_idx, pair.to_idx));
        raw.pairs[0].value = f32::NAN;
        assert!(matches!(
            raw.validate(),
            Err(FactionSentimentValidationError::NonFiniteValue { .. })
        ));

        let zero_baseline = faction_full_baseline(0, Vec::new());
        assert_eq!(
            zero_baseline.validate(),
            Err(FactionSentimentValidationError::ZeroBaselineId {
                kind: FACTION_SENTIMENT_RECORD_KIND_FULL_BASELINE,
            })
        );

        let zero_ref = RawFactionSentimentRecord {
            kind: FACTION_SENTIMENT_RECORD_KIND_DELTA,
            has_baseline_ref: true,
            baseline_ref: 0,
            baseline_id: 8,
            pairs: Vec::new(),
        };
        assert_eq!(
            zero_ref.validate(),
            Err(FactionSentimentValidationError::ZeroBaselineRef {
                kind: FACTION_SENTIMENT_RECORD_KIND_DELTA,
            })
        );
    }

    #[test]
    fn faction_sentiment_record_rejects_pair_count_above_wire_capacity() {
        let pairs = (0..=MAX_FACTION_SENTIMENT_PAIRS)
            .map(|to_idx| FactionSentimentPair {
                from_idx: 0,
                to_idx: u16::try_from(to_idx).unwrap(),
                value: -0.5,
            })
            .collect();
        let raw = faction_full_baseline(7, pairs);

        assert_eq!(
            raw.validate(),
            Err(FactionSentimentValidationError::TooManyPairs {
                count: MAX_FACTION_SENTIMENT_PAIRS + 1,
            })
        );
    }

    // --- Validation: happy paths ---

    #[test]
    fn validate_full_baseline_produces_typed_record() {
        let schema = schema_with(vec![number_descriptor(3)]);
        let raw = full_baseline(3, 9, WireSlotValue::Number(50.0));
        assert_eq!(
            raw.validate(&schema),
            Ok(StateSlotRecord::FullBaseline {
                slot_id: StateSlotId(3),
                baseline_id: 9,
                value: WireSlotValue::Number(50.0),
            })
        );
    }

    #[test]
    fn validate_delta_maps_baseline_ids() {
        let schema = schema_with(vec![number_descriptor(3)]);
        let raw = delta(3, 9, 10, WireSlotValue::Number(60.0));
        assert_eq!(
            raw.validate(&schema),
            Ok(StateSlotRecord::Delta {
                slot_id: StateSlotId(3),
                baseline_ref: 9,
                new_baseline_id: 10,
                value: WireSlotValue::Number(60.0),
            })
        );
    }

    #[test]
    fn unset_is_valid_against_any_type() {
        let schema = schema_with(vec![number_descriptor(3), enum_descriptor(4)]);
        assert!(
            full_baseline(3, 1, WireSlotValue::Unset)
                .validate(&schema)
                .is_ok()
        );
        assert!(
            full_baseline(4, 1, WireSlotValue::Unset)
                .validate(&schema)
                .is_ok()
        );
    }

    #[test]
    fn validate_batch_checks_fingerprint_and_records() {
        let schema = schema_with(vec![number_descriptor(3)]);
        let records = vec![full_baseline(3, 1, WireSlotValue::Number(10.0))];
        let typed = validate_state_records(&schema, &[7u8; 32], &records).expect("batch validates");
        assert_eq!(typed.len(), 1);
    }

    // --- Validation: malformed rejections ---

    #[test]
    fn fingerprint_mismatch_rejects_before_records() {
        let schema = schema_with(vec![number_descriptor(3)]);
        let records = vec![full_baseline(3, 1, WireSlotValue::Number(10.0))];
        assert_eq!(
            validate_state_records(&schema, &[0u8; 32], &records),
            Err(StateValidationError::SchemaFingerprintMismatch)
        );
    }

    #[test]
    fn over_cap_record_count_rejects() {
        let schema = schema_with(vec![number_descriptor(3)]);
        let records = vec![
            full_baseline(3, 1, WireSlotValue::Number(1.0));
            MAX_STATE_RECORDS_PER_SNAPSHOT + 1
        ];
        assert_eq!(
            validate_state_records(&schema, &[7u8; 32], &records),
            Err(StateValidationError::TooManyStateRecords {
                count: MAX_STATE_RECORDS_PER_SNAPSHOT + 1,
            })
        );
    }

    #[test]
    fn unknown_slot_id_rejects() {
        let schema = schema_with(vec![number_descriptor(3)]);
        let raw = full_baseline(99, 1, WireSlotValue::Number(1.0));
        assert_eq!(
            raw.validate(&schema),
            Err(StateValidationError::UnknownSlotId(99))
        );
    }

    #[test]
    fn type_mismatch_rejects() {
        let schema = schema_with(vec![number_descriptor(3)]);
        let raw = full_baseline(3, 1, WireSlotValue::Boolean(true));
        assert_eq!(
            raw.validate(&schema),
            Err(StateValidationError::TypeMismatch { slot_id: 3 })
        );
    }

    #[test]
    fn non_finite_number_rejects() {
        let schema = schema_with(vec![number_descriptor(3)]);
        let raw = full_baseline(3, 1, WireSlotValue::Number(f32::NAN));
        assert_eq!(
            raw.validate(&schema),
            Err(StateValidationError::NonFiniteValue { slot_id: 3 })
        );
    }

    #[test]
    fn non_finite_array_element_rejects() {
        let schema = schema_with(vec![array_descriptor(8)]);
        let raw = full_baseline(8, 1, WireSlotValue::Array(vec![1.0, f32::INFINITY]));
        assert_eq!(
            raw.validate(&schema),
            Err(StateValidationError::NonFiniteValue { slot_id: 8 })
        );
    }

    #[test]
    fn over_cap_string_rejects() {
        let schema = schema_with(vec![StateSlotDescriptor {
            slot_id: StateSlotId(6),
            value_type: SlotValueType::String,
            range: None,
            scope: ReplicationScope::SharedGlobal,
        }]);
        let too_long = "x".repeat(MAX_STRING_BYTES + 1);
        let raw = full_baseline(6, 1, WireSlotValue::String(too_long));
        assert_eq!(
            raw.validate(&schema),
            Err(StateValidationError::StringTooLong {
                slot_id: 6,
                bytes: MAX_STRING_BYTES + 1,
            })
        );
    }

    #[test]
    fn over_cap_array_rejects() {
        let schema = schema_with(vec![array_descriptor(8)]);
        let too_long = vec![0.0_f32; MAX_ARRAY_ELEMENTS + 1];
        let raw = full_baseline(8, 1, WireSlotValue::Array(too_long));
        assert_eq!(
            raw.validate(&schema),
            Err(StateValidationError::ArrayTooLong {
                slot_id: 8,
                elements: MAX_ARRAY_ELEMENTS + 1,
            })
        );
    }

    #[test]
    fn unknown_enum_value_rejects() {
        let schema = schema_with(vec![enum_descriptor(4)]);
        let raw = full_baseline(4, 1, WireSlotValue::Enum("hostile".to_string()));
        assert_eq!(
            raw.validate(&schema),
            Err(StateValidationError::UnknownEnumValue { slot_id: 4 })
        );
    }

    #[test]
    fn declared_enum_value_validates() {
        let schema = schema_with(vec![enum_descriptor(4)]);
        let raw = full_baseline(4, 1, WireSlotValue::Enum("focus".to_string()));
        assert!(raw.validate(&schema).is_ok());
    }

    // --- Validation: illegal baseline combinations ---

    #[test]
    fn full_baseline_with_ref_rejects() {
        let schema = schema_with(vec![number_descriptor(3)]);
        let raw = RawStateSlotRecord {
            slot_id: 3,
            kind: STATE_RECORD_KIND_FULL_BASELINE,
            has_baseline_ref: true,
            baseline_ref: 5,
            baseline_id: 9,
            value: WireSlotValue::Number(1.0),
        };
        assert_eq!(
            raw.validate(&schema),
            Err(StateValidationError::BadBaselineCombination {
                kind: STATE_RECORD_KIND_FULL_BASELINE,
                has_ref: true,
            })
        );
    }

    #[test]
    fn delta_without_ref_rejects() {
        let schema = schema_with(vec![number_descriptor(3)]);
        let raw = RawStateSlotRecord {
            slot_id: 3,
            kind: STATE_RECORD_KIND_DELTA,
            has_baseline_ref: false,
            baseline_ref: 0,
            baseline_id: 9,
            value: WireSlotValue::Number(1.0),
        };
        assert_eq!(
            raw.validate(&schema),
            Err(StateValidationError::BadBaselineCombination {
                kind: STATE_RECORD_KIND_DELTA,
                has_ref: false,
            })
        );
    }

    #[test]
    fn absent_flag_with_nonzero_ref_rejects() {
        let schema = schema_with(vec![number_descriptor(3)]);
        let raw = RawStateSlotRecord {
            slot_id: 3,
            kind: STATE_RECORD_KIND_DELTA,
            has_baseline_ref: false,
            baseline_ref: 7,
            baseline_id: 9,
            value: WireSlotValue::Number(1.0),
        };
        assert_eq!(
            raw.validate(&schema),
            Err(StateValidationError::BadBaselineCombination {
                kind: STATE_RECORD_KIND_DELTA,
                has_ref: false,
            })
        );
    }

    #[test]
    fn unknown_record_kind_rejects() {
        let schema = schema_with(vec![number_descriptor(3)]);
        let raw = RawStateSlotRecord {
            slot_id: 3,
            kind: 99,
            has_baseline_ref: false,
            baseline_ref: 0,
            baseline_id: 9,
            value: WireSlotValue::Number(1.0),
        };
        assert_eq!(
            raw.validate(&schema),
            Err(StateValidationError::UnknownRecordKind(99))
        );
    }

    #[test]
    fn first_bad_record_short_circuits_batch() {
        let schema = schema_with(vec![number_descriptor(3)]);
        let records = vec![
            full_baseline(3, 1, WireSlotValue::Number(10.0)),
            full_baseline(99, 1, WireSlotValue::Number(10.0)),
        ];
        assert_eq!(
            validate_state_records(&schema, &[7u8; 32], &records),
            Err(StateValidationError::UnknownSlotId(99))
        );
    }

    // --- Value-kind discriminants pinned ---

    #[test]
    fn value_kind_discriminants_pinned() {
        assert_eq!(WireSlotValue::Unset.kind(), VALUE_KIND_UNSET);
        assert_eq!(WireSlotValue::Number(0.0).kind(), VALUE_KIND_NUMBER);
        assert_eq!(WireSlotValue::Boolean(false).kind(), VALUE_KIND_BOOLEAN);
        assert_eq!(
            WireSlotValue::String(String::new()).kind(),
            VALUE_KIND_STRING
        );
        assert_eq!(WireSlotValue::Enum(String::new()).kind(), VALUE_KIND_ENUM);
        assert_eq!(WireSlotValue::Array(Vec::new()).kind(), VALUE_KIND_ARRAY);
    }
}
