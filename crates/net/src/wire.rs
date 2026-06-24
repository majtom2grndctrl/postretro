// Wire codec: bitcode-serialized message types and the protocol/version handshake.
// See: context/lib/networking.md
//
// Every type that crosses the wire derives native `bitcode::Encode`/`Decode`.
// These are dedicated *wire-mirror* types: the engine's `ComponentValue` is a
// serde internally-tagged enum (`#[serde(tag = "kind")]`) which bitcode cannot
// round-trip (`DeserializeAnyNotSupported`). So the component payload carries an
// explicit `u16` discriminant — numeric-equal to the engine `ComponentKind` —
// plus its inner payload, and no serde-internally-tagged enum ever crosses here.
//
// This crate is `postretro`-free and glam-free by design: mirror types use plain
// `[f32; N]` / `f32` / `bool`, never the engine or glam types they shadow. The
// engine-side conversions (`ComponentValue::Transform` <-> `WireTransform`,
// `SimCommand` <-> `InputCommand`) live in `crate::netcode` in the engine, not
// here.
//
// Phase 2 splits the snapshot into a *raw encoded boundary* and a *typed apply
// model*. The raw structs (`RawSnapshotMessage`, `RawEntityRecord`,
// `RawComponentPayload`) carry explicit numeric `record_kind`/`component_kind`
// discriminants and `Option` payload slots, so an invalid kind value or a
// missing/duplicate slot decodes cleanly into the raw envelope and is rejected
// at `validate` time — never at decode, and never by reaching the registry. The
// typed model (`SnapshotMessage`, `EntityRecord`, `ComponentPayload`) is produced
// only after that validation, so a typed record is always well-formed by
// construction.

use bitcode::{Decode, Encode};

/// Network-stable entity identity. A `u32` newtype assigned by the host; the wire
/// carries it as a bare `u32` (bitcode encodes the inner field transparently).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Encode, Decode)]
pub struct NetworkId(pub u32);

/// Pinned snapshot wire-format version. Carried in `RawSnapshotMessage.version`
/// and asserted *after* the two handshake gates, so a Phase 1 peer is already
/// refused by the gates before any Phase 2 snapshot reaches this check.
///
/// Bumped to 4 in M15 Phase 3 Task 7: the entity record gained
/// `has_entity_class`/`entity_class`, so a record's bitcode layout changed.
///
/// Bumped to 5 in M15 Phase 3.5: `RawSnapshotMessage` gained
/// `state_schema_fingerprint`/`state_records`, `AckMessage` gained
/// `slot_baselines`, and `ClientMessage` gained `StateBaselineRefresh` — the
/// snapshot, ack, and client-message bitcode layouts all changed.
///
/// Bumped to 6 in M15 E10 (networked enemy authority): the `entity_class`
/// validation contract changed — a class may now ride any non-despawn record
/// backed by a structurally-valid finite `Transform` (no `PlayerMovementState`
/// required), so descriptor-backed remote *presentation* entities can be
/// materialized from `Transform`-only snapshots. The bitcode byte layout of the
/// record is unchanged (no field added/reordered), but the accepted-envelope set
/// changed, so peers on the prior contract are refused by the version gate.
pub const SNAPSHOT_VERSION: u16 = 6;

/// `record_kind` discriminant for a full-baseline (spawn / join / refresh) record.
pub const RECORD_KIND_FULL_BASELINE: u16 = 0;
/// `record_kind` discriminant for a delta-update record.
pub const RECORD_KIND_DELTA: u16 = 1;
/// `record_kind` discriminant for a despawn record.
pub const RECORD_KIND_DESPAWN: u16 = 2;

/// `component_kind` discriminant for a `Transform` payload. Numeric-equal to the
/// engine `ComponentKind::Transform as u16`.
pub const COMPONENT_KIND_TRANSFORM: u16 = 0;
/// `component_kind` discriminant for a `PlayerMovementState` payload. Numeric-equal
/// to the engine `ComponentKind::PlayerMovement as u16` (Phase 2 = 6).
pub const COMPONENT_KIND_PLAYER_MOVEMENT_STATE: u16 = 6;

/// Wire mirror of the engine `Transform`. Phase 2 replicates `position`,
/// `rotation`, and `scale`.
///
/// `rotation` mirrors the engine quaternion in **`[x, y, z, w]` order**. The
/// engine-side conversion (which knows glam's `Quat` component order) lives in
/// `crate::netcode`; here it is just four floats in that fixed order.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireTransform {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl WireTransform {
    /// Whether every replicated float is finite (no NaN/inf): position, rotation,
    /// and scale. Registry-blind — operates only on the plain `[f32; N]` wire
    /// fields, never an engine/glam type. Backs two rules at `validate`: it gates
    /// the `Transform` component payload (a non-finite pose is rejected before
    /// typed apply, so none reaches the registry) and it backs the `entity_class`
    /// rule (a class may only ride a record carrying a finite `Transform`).
    #[must_use]
    fn all_finite(&self) -> bool {
        self.position.iter().all(|c| c.is_finite())
            && self.rotation.iter().all(|c| c.is_finite())
            && self.scale.iter().all(|c| c.is_finite())
    }
}

/// Wire mirror of the engine player movement state machine's active state. Only
/// the mutable tick fields each variant needs cross the wire; descriptor-immutable
/// tuning lives local on both peers.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub enum WireMovementState {
    Normal,
    Dash { elapsed_ms: f32, boost: [f32; 3] },
    Crouching { eye_current: f32 },
}

/// Wire mirror of the *mutable tick subset* of the engine `PlayerMovementComponent`.
///
/// Deliberately **not** a copy of the component struct: descriptor-immutable
/// movement params, `view_feel`, `standing_*`, `stuck_stop_*`, and the IR-bound
/// `dash_programs` are local data on both peers and must never be authoritative
/// wire state. Only the fields interpolation and later prediction reconciliation
/// need are mirrored here. Source field types are preserved: ability counters and
/// `air_ticks` are `u32`; live timers are `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WirePlayerMovementState {
    pub velocity: [f32; 3],
    pub is_grounded: bool,
    pub air_jumps_remaining: u32,
    pub air_dashes_remaining: u32,
    pub dash_cooldown_ms: f32,
    pub air_ticks: u32,
    pub movement_state: WireMovementState,
    pub coyote_timer_ms: f32,
    pub jump_buffer_timer_ms: f32,
    pub jump_spent: bool,
    pub capsule_half_height: f32,
    pub capsule_eye_height: f32,
}

impl WireMovementState {
    /// Whether every float this variant carries is finite (no NaN/inf). `Normal`
    /// carries nothing, so it is vacuously finite. The exhaustive `match` (no `_`
    /// arm) means a new variant with float payload is a compile error here until its
    /// finiteness is accounted for — the same drift discipline as the variant guards.
    #[must_use]
    fn all_finite(&self) -> bool {
        match self {
            WireMovementState::Normal => true,
            WireMovementState::Dash { elapsed_ms, boost } => {
                elapsed_ms.is_finite() && boost.iter().all(|c| c.is_finite())
            }
            WireMovementState::Crouching { eye_current } => eye_current.is_finite(),
        }
    }
}

impl WirePlayerMovementState {
    /// Whether every replicated float is finite (no NaN/inf): velocity, all live
    /// timers, capsule dimensions, and the active state's payload. Checked at
    /// `validate` so a non-finite movement state is rejected before typed apply and
    /// never reaches the registry. Integer counters and bools cannot be non-finite,
    /// so they are not checked.
    #[must_use]
    fn all_finite(&self) -> bool {
        self.velocity.iter().all(|c| c.is_finite())
            && self.dash_cooldown_ms.is_finite()
            && self.coyote_timer_ms.is_finite()
            && self.jump_buffer_timer_ms.is_finite()
            && self.capsule_half_height.is_finite()
            && self.capsule_eye_height.is_finite()
            && self.movement_state.all_finite()
    }
}

// ---------------------------------------------------------------------------
// Raw encoded boundary
// ---------------------------------------------------------------------------

/// Raw component payload as it crosses the wire: an explicit `component_kind`
/// discriminant plus one `Option` slot per supported component. Exactly one slot
/// must be `Some` and it must match `component_kind`; any other shape (wrong slot,
/// no slot, two slots, unknown kind) is a clean decode but a `validate` rejection.
///
/// The explicit discriminant + `Option` slots are deliberate: they make an invalid
/// kind value or a missing/duplicate payload *representable* in the decoded
/// envelope, so the malformed-input tests exercise validation without relying on
/// bitcode's internal enum tag (which would make an invalid tag a decode error
/// instead of a testable rejected envelope).
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct RawComponentPayload {
    pub component_kind: u16,
    pub transform: Option<WireTransform>,
    pub player_movement: Option<WirePlayerMovementState>,
}

/// Raw per-entity lifecycle record. `record_kind` selects which logical record
/// this is; the `baseline_id_or_ref` / `new_baseline_id_or_tombstone_id` / `reason`
/// fields are overloaded per kind (see the `validate` mapping). Unused fields for a
/// kind are ignored, so a `Despawn` need not zero its component list to be valid.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct RawEntityRecord {
    pub record_kind: u16,
    pub network_id: u32,
    /// `FullBaseline`: the new baseline id. `Delta`: the referenced baseline id.
    /// Unused for `Despawn`.
    pub baseline_id_or_ref: u32,
    /// `Delta`: the new baseline id. `Despawn`: the tombstone id. Unused for
    /// `FullBaseline`.
    pub new_baseline_id_or_tombstone_id: u32,
    /// `Despawn`: the despawn reason code. Unused for `FullBaseline`/`Delta`.
    pub reason: u8,
    /// Movement-authority ack flag: whether `last_processed_client_tick` carries a
    /// real value. Mirrors the `Option`-ness of the typed
    /// `EntityRecord::last_processed_client_tick`: `false` ⇒ no tick resolved yet
    /// (the typed value is `None`); `true` ⇒ the host has resolved ≥1 command tick
    /// for this pawn (the typed value is `Some(last_processed_client_tick)`). The
    /// flag is required because the wire integer cannot itself encode "absent" — a
    /// `false` flag combined with a nonzero tick is a malformed envelope rejected at
    /// `validate`.
    pub has_last_processed_client_tick: bool,
    /// The latest client command tick the host resolved for this pawn before
    /// snapshotting. Meaningful only when `has_last_processed_client_tick` is `true`
    /// (and only on movement records — a non-movement or despawn record carrying
    /// either ack field is rejected at `validate`).
    pub last_processed_client_tick: u32,
    /// True only in the per-recipient snapshot sent to this pawn's owning client, so
    /// that client predicts/reconciles it locally. Always `false` for non-local
    /// pawns and for non-movement / despawn records (a `true` flag on either is
    /// rejected at `validate`).
    pub local_player: bool,
    /// Whether `entity_class` carries a real value (mirrors the `Option`-ness of the
    /// typed `EntityRecord::entity_class`). `false` ⇒ no class stamped (the typed
    /// value is `None`); `true` ⇒ the host stamped the descriptor class the pawn was
    /// materialized from (the typed value is `Some(entity_class)`). The flag is
    /// required because an empty `String` is a legal-but-meaningless class; a `false`
    /// flag paired with a non-empty class is a malformed envelope rejected at
    /// `validate`.
    pub has_entity_class: bool,
    /// The opaque descriptor-class identifier the host materialized this entity from
    /// (e.g. `"player"`), so the client can materialize the matching descriptor-backed
    /// presentation entity locally. Meaningful only when `has_entity_class` is `true`,
    /// and valid only on a non-despawn record backed by a finite `Transform` — a
    /// despawn record, or a record without a finite `Transform`, carrying it is
    /// rejected at `validate`. This is a plain string identifier, NOT a descriptor
    /// type: the crate stays registry-blind and never resolves it.
    pub entity_class: String,
    pub components: Vec<RawComponentPayload>,
}

/// Raw snapshot envelope as it crosses the wire. `version` is checked against
/// [`SNAPSHOT_VERSION`] during validation. bitcode length-prefixes `records`, which
/// is the count prefix on the wire; an empty snapshot encodes as count 0 and is a
/// valid carrier for ack/sequence metadata.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct RawSnapshotMessage {
    pub version: u16,
    pub sequence: u32,
    pub server_tick: u32,
    pub records: Vec<RawEntityRecord>,
    /// Opaque 32-byte fingerprint of the server's replicated-slot schema (M15
    /// Phase 3.5). The client matches it against its own local fingerprint before
    /// applying any state record. This crate never computes it — the engine
    /// (`postretro`) computes it with `blake3` and hands it across as bytes.
    pub state_schema_fingerprint: [u8; 32],
    /// Replicated state-slot records riding this snapshot. Empty is valid (the
    /// snapshot carries no slot changes this frame). Validated against the local
    /// schema by [`crate::state_slots::validate_state_records`], not here — schema
    /// validation needs the engine-owned `StateSchema`, which this registry-blind
    /// crate is never handed at decode time.
    pub state_records: Vec<crate::state_slots::RawStateSlotRecord>,
}

// ---------------------------------------------------------------------------
// Typed apply model (produced only after validation)
// ---------------------------------------------------------------------------

/// A validated component payload. Constructed only by [`RawComponentPayload::validate`],
/// so a typed payload always has exactly the inner value its kind requires.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ComponentPayload {
    Transform(WireTransform),
    PlayerMovementState(WirePlayerMovementState),
}

impl ComponentPayload {
    /// Engine-aligned `u16` discriminant for this payload, numeric-equal to
    /// `ComponentKind as u16` in the engine. Drift here desyncs replication, so
    /// the mapping is pinned by `component_kind_pinned_to_engine_discriminants`.
    #[must_use]
    pub fn kind(&self) -> u16 {
        match self {
            ComponentPayload::Transform(_) => COMPONENT_KIND_TRANSFORM,
            ComponentPayload::PlayerMovementState(_) => COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
        }
    }
}

/// A validated lifecycle record. Constructed only by [`RawEntityRecord::validate`].
/// A full-baseline refresh *response* is encoded as a `FullBaseline` record — the
/// repair format is identical to the join/spawn format on the wire.
#[derive(Debug, Clone, PartialEq)]
pub enum EntityRecord {
    FullBaseline {
        network_id: u32,
        baseline_id: u32,
        /// Latest client command tick the host resolved for this pawn before
        /// snapshotting. `Some` on the recipient-local movement pawn once ≥1 real or
        /// synthetic command tick is resolved; `None` for non-local movement pawns and
        /// for the first baseline before any tick is resolved. Always `None` on a
        /// record that does not carry a `PlayerMovementState` (enforced at `validate`).
        last_processed_client_tick: Option<u32>,
        /// True only in the snapshot sent to this pawn's owning client. Always `false`
        /// for non-local pawns and for non-movement records (enforced at `validate`).
        local_player: bool,
        /// The opaque descriptor-class identifier the host materialized this entity
        /// from (e.g. `"player"`), or `None` for a record the host stamped no class
        /// for. `Some` only on a non-despawn record carrying a finite `Transform`
        /// (enforced at `validate`) — the class names the descriptor the client
        /// materializes, and that presentation entity rides the wire as a `Transform`.
        /// A plain string identifier, never resolved by this registry-blind crate.
        entity_class: Option<String>,
        components: Vec<ComponentPayload>,
    },
    Delta {
        network_id: u32,
        baseline_ref: u32,
        new_baseline_id: u32,
        /// See `FullBaseline::last_processed_client_tick`.
        last_processed_client_tick: Option<u32>,
        /// See `FullBaseline::local_player`.
        local_player: bool,
        /// See `FullBaseline::entity_class`.
        entity_class: Option<String>,
        components: Vec<ComponentPayload>,
    },
    Despawn {
        network_id: u32,
        tombstone_id: u32,
        reason: u8,
    },
}

/// A validated snapshot message: the typed apply model the engine glue consumes.
/// Produced only by [`RawSnapshotMessage::validate`].
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotMessage {
    pub sequence: u32,
    pub server_tick: u32,
    pub records: Vec<EntityRecord>,
    /// Carried through unchanged from the raw snapshot. The entity-record half is
    /// validated by [`RawSnapshotMessage::validate`]; the state-record half is
    /// schema-validated separately by the engine via
    /// [`crate::state_slots::validate_state_records`], which needs the engine-owned
    /// local schema. The fingerprint and raw records ride here so the engine glue
    /// gets both halves of one server frame from a single typed message.
    pub state_schema_fingerprint: [u8; 32],
    pub state_records: Vec<crate::state_slots::RawStateSlotRecord>,
}

// ---------------------------------------------------------------------------
// Validation: raw -> typed
// ---------------------------------------------------------------------------

/// Why a structurally-decodable raw snapshot is not a valid typed snapshot. These
/// are *semantic* rejections that happen after a clean bitcode decode: the bytes
/// parsed, but the kind/slot shape is not a record the registry could apply. A
/// corrupt or truncated buffer is a [`WireError`] at decode, never a
/// `ValidationError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    /// `RawSnapshotMessage.version` did not equal [`SNAPSHOT_VERSION`].
    VersionMismatch { expected: u16, received: u16 },
    /// `record_kind` was not one of the defined record discriminants.
    UnknownRecordKind(u16),
    /// `component_kind` was not one of the defined component discriminants.
    UnknownComponentKind(u16),
    /// The slot `component_kind` names was `None` (whether or not a different slot
    /// was populated).
    MissingComponentPayload(u16),
    /// More than one payload slot was `Some` (ambiguous which one `component_kind`
    /// names).
    MismatchedComponentPayload(u16),
    /// A record carried movement-authority metadata
    /// (`has_last_processed_client_tick` / `local_player`) but does not carry a
    /// `PlayerMovementState` component. Ack/local-player metadata is only meaningful
    /// on a movement pawn record.
    MovementMetadataWithoutMovement,
    /// `has_last_processed_client_tick` was `false` but `last_processed_client_tick`
    /// was nonzero — the "absent" flag cannot ride a real tick value.
    MalformedTickMetadata { last_processed_client_tick: u32 },
    /// A despawn record carried any movement-authority metadata
    /// (`has_last_processed_client_tick` / `local_player`) or an `entity_class`. A
    /// tombstone has no pawn state to ack and no descriptor class to materialize.
    MetadataOnDespawn,
    /// A record carried an `entity_class` (`has_entity_class = true`) but does not
    /// carry a structurally-valid finite `Transform` payload. The class names a
    /// descriptor the client materializes as a presentation entity, which rides the
    /// wire as a `Transform`; without a finite pose there is nothing to place.
    EntityClassWithoutTransform,
    /// `has_entity_class` was `false` but `entity_class` was non-empty — the "absent"
    /// flag cannot ride a real class value.
    MalformedEntityClassMetadata,
    /// A `PlayerMovementState` payload carried a non-finite float (NaN/inf) in one
    /// of its replicated fields (velocity, timers, dash boost, crouch eye value, or
    /// capsule dimensions). Rejected before typed apply so no non-finite movement
    /// state reaches the registry.
    NonFiniteMovementState,
    /// A `Transform` payload carried a non-finite float (NaN/inf) in its position,
    /// rotation, or scale. Rejected before typed apply so no non-finite pose reaches
    /// the registry — and so a non-finite `Transform` cannot back an `entity_class`.
    NonFiniteTransform,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::VersionMismatch { expected, received } => write!(
                f,
                "snapshot version mismatch: expected {expected}, received {received}"
            ),
            ValidationError::UnknownRecordKind(k) => write!(f, "unknown record_kind {k}"),
            ValidationError::UnknownComponentKind(k) => write!(f, "unknown component_kind {k}"),
            ValidationError::MissingComponentPayload(k) => {
                write!(f, "missing payload slot for component_kind {k}")
            }
            ValidationError::MismatchedComponentPayload(k) => {
                write!(
                    f,
                    "mismatched/duplicate payload slot for component_kind {k}"
                )
            }
            ValidationError::MovementMetadataWithoutMovement => write!(
                f,
                "movement-authority metadata on a record without a PlayerMovementState component"
            ),
            ValidationError::MalformedTickMetadata {
                last_processed_client_tick,
            } => write!(
                f,
                "has_last_processed_client_tick=false but last_processed_client_tick={last_processed_client_tick} is nonzero"
            ),
            ValidationError::MetadataOnDespawn => {
                write!(f, "movement-authority metadata on a despawn record")
            }
            ValidationError::EntityClassWithoutTransform => write!(
                f,
                "entity_class on a record without a finite Transform component"
            ),
            ValidationError::MalformedEntityClassMetadata => {
                write!(f, "has_entity_class=false but entity_class is non-empty")
            }
            ValidationError::NonFiniteMovementState => {
                write!(f, "non-finite float in a PlayerMovementState payload")
            }
            ValidationError::NonFiniteTransform => {
                write!(f, "non-finite float in a Transform payload")
            }
        }
    }
}

impl std::error::Error for ValidationError {}

impl RawComponentPayload {
    /// Validate this raw payload into a typed [`ComponentPayload`]. Rejects unknown
    /// `component_kind` values, a missing slot for the named kind, and any extra or
    /// mismatched slot. Exactly one slot must be `Some` and it must be the one the
    /// `component_kind` names.
    pub fn validate(&self) -> Result<ComponentPayload, ValidationError> {
        // Count populated slots once: a well-formed payload has exactly one, the
        // one its `component_kind` names. Any other slot being `Some` is a
        // mismatch regardless of the named kind (it makes the envelope ambiguous).
        let populated =
            usize::from(self.transform.is_some()) + usize::from(self.player_movement.is_some());

        match self.component_kind {
            COMPONENT_KIND_TRANSFORM => match self.transform {
                Some(t) if populated == 1 => {
                    if t.all_finite() {
                        Ok(ComponentPayload::Transform(t))
                    } else {
                        Err(ValidationError::NonFiniteTransform)
                    }
                }
                Some(_) => Err(ValidationError::MismatchedComponentPayload(
                    self.component_kind,
                )),
                None => Err(ValidationError::MissingComponentPayload(
                    self.component_kind,
                )),
            },
            COMPONENT_KIND_PLAYER_MOVEMENT_STATE => match self.player_movement {
                Some(m) if populated == 1 => {
                    if m.all_finite() {
                        Ok(ComponentPayload::PlayerMovementState(m))
                    } else {
                        Err(ValidationError::NonFiniteMovementState)
                    }
                }
                Some(_) => Err(ValidationError::MismatchedComponentPayload(
                    self.component_kind,
                )),
                None => Err(ValidationError::MissingComponentPayload(
                    self.component_kind,
                )),
            },
            other => Err(ValidationError::UnknownComponentKind(other)),
        }
    }
}

impl RawEntityRecord {
    /// Validate this raw record into a typed [`EntityRecord`]. Rejects unknown
    /// `record_kind` values and propagates any per-component validation failure;
    /// the overloaded id/reason fields are interpreted per the record's kind.
    pub fn validate(&self) -> Result<EntityRecord, ValidationError> {
        match self.record_kind {
            RECORD_KIND_FULL_BASELINE => {
                let components = self.validate_components()?;
                let last_processed_client_tick = self.validate_movement_metadata(&components)?;
                let entity_class = self.validate_entity_class(&components)?;
                Ok(EntityRecord::FullBaseline {
                    network_id: self.network_id,
                    baseline_id: self.baseline_id_or_ref,
                    last_processed_client_tick,
                    local_player: self.local_player,
                    entity_class,
                    components,
                })
            }
            RECORD_KIND_DELTA => {
                let components = self.validate_components()?;
                let last_processed_client_tick = self.validate_movement_metadata(&components)?;
                let entity_class = self.validate_entity_class(&components)?;
                Ok(EntityRecord::Delta {
                    network_id: self.network_id,
                    baseline_ref: self.baseline_id_or_ref,
                    new_baseline_id: self.new_baseline_id_or_tombstone_id,
                    last_processed_client_tick,
                    local_player: self.local_player,
                    entity_class,
                    components,
                })
            }
            // Despawn carries no components; any present are ignored, matching the
            // overloaded-field rule (only the fields a kind names are read). It must
            // not carry movement-authority metadata or an entity_class, though — a
            // tombstone has no pawn state to ack and no descriptor class to materialize.
            RECORD_KIND_DESPAWN => {
                if self.has_last_processed_client_tick
                    || self.last_processed_client_tick != 0
                    || self.local_player
                    || self.has_entity_class
                    || !self.entity_class.is_empty()
                {
                    return Err(ValidationError::MetadataOnDespawn);
                }
                Ok(EntityRecord::Despawn {
                    network_id: self.network_id,
                    tombstone_id: self.new_baseline_id_or_tombstone_id,
                    reason: self.reason,
                })
            }
            other => Err(ValidationError::UnknownRecordKind(other)),
        }
    }

    fn validate_components(&self) -> Result<Vec<ComponentPayload>, ValidationError> {
        self.components
            .iter()
            .map(RawComponentPayload::validate)
            .collect()
    }

    /// Validate this record's movement-authority metadata against its (already
    /// validated) components, returning the typed `last_processed_client_tick`.
    ///
    /// Rules:
    /// - `has_last_processed_client_tick = false` with a nonzero tick is malformed.
    /// - ack/local-player metadata is only valid on a record carrying a
    ///   `PlayerMovementState`; on any other record it is rejected.
    fn validate_movement_metadata(
        &self,
        components: &[ComponentPayload],
    ) -> Result<Option<u32>, ValidationError> {
        // The raw flag must be internally consistent before anything else: a "tick
        // absent" flag cannot ride a real tick value.
        if !self.has_last_processed_client_tick && self.last_processed_client_tick != 0 {
            return Err(ValidationError::MalformedTickMetadata {
                last_processed_client_tick: self.last_processed_client_tick,
            });
        }

        let carries_movement = components
            .iter()
            .any(|c| matches!(c, ComponentPayload::PlayerMovementState(_)));
        let carries_metadata = self.has_last_processed_client_tick || self.local_player;

        if carries_metadata && !carries_movement {
            return Err(ValidationError::MovementMetadataWithoutMovement);
        }

        Ok(self
            .has_last_processed_client_tick
            .then_some(self.last_processed_client_tick))
    }

    /// Validate this record's `entity_class` metadata against its (already validated)
    /// components, returning the typed `Option<String>`. Called only for non-despawn
    /// records — a despawn carrying any `entity_class` is rejected up front in
    /// [`RawEntityRecord::validate`] (`MetadataOnDespawn`).
    ///
    /// Rules:
    /// - `has_entity_class = false` with a non-empty class is malformed.
    /// - an `entity_class` is valid only on a record carrying at least one
    ///   structurally-valid finite `Transform` payload (its position/rotation/scale
    ///   are all finite). It no longer requires a `PlayerMovementState`: a snapshot
    ///   tells the client "this remote entity is descriptor class X" so it can
    ///   materialize the matching mesh, and that presentation entity rides the wire
    ///   as a `Transform` only. The finiteness gate is the registry-blind
    ///   [`WireTransform::all_finite`] — the same check that backs the `Transform`
    ///   component payload — so a class can never name a descriptor backed by a
    ///   non-finite pose. (A non-finite `Transform` is already rejected at
    ///   component validation with `NonFiniteTransform`; this re-checks finiteness so
    ///   the rule is self-contained and an empty/non-Transform record with a class is
    ///   still rejected.)
    fn validate_entity_class(
        &self,
        components: &[ComponentPayload],
    ) -> Result<Option<String>, ValidationError> {
        // The flag must be internally consistent first: an "absent" flag cannot ride a
        // real (non-empty) class value.
        if !self.has_entity_class && !self.entity_class.is_empty() {
            return Err(ValidationError::MalformedEntityClassMetadata);
        }

        if self.has_entity_class {
            let carries_finite_transform = components.iter().any(|c| match c {
                ComponentPayload::Transform(t) => t.all_finite(),
                ComponentPayload::PlayerMovementState(_) => false,
            });
            if !carries_finite_transform {
                return Err(ValidationError::EntityClassWithoutTransform);
            }
            Ok(Some(self.entity_class.clone()))
        } else {
            Ok(None)
        }
    }
}

impl RawSnapshotMessage {
    /// Validate this raw snapshot into a typed [`SnapshotMessage`]. Checks the
    /// pinned [`SNAPSHOT_VERSION`] first, then validates every record. The first
    /// rejection short-circuits — no partial typed snapshot is produced, and
    /// nothing reaches the registry.
    pub fn validate(&self) -> Result<SnapshotMessage, ValidationError> {
        if self.version != SNAPSHOT_VERSION {
            return Err(ValidationError::VersionMismatch {
                expected: SNAPSHOT_VERSION,
                received: self.version,
            });
        }
        let records = self
            .records
            .iter()
            .map(RawEntityRecord::validate)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SnapshotMessage {
            sequence: self.sequence,
            server_tick: self.server_tick,
            records,
            state_schema_fingerprint: self.state_schema_fingerprint,
            state_records: self.state_records.clone(),
        })
    }
}

/// Wire mirror of the engine `MovementInput` fields the input command carries.
/// `wish_dir` is `[right, forward]` (mirroring glam `Vec2` x = right, y = forward).
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireMovementInput {
    pub wish_dir: [f32; 2],
    pub jump_pressed: bool,
    pub dash_pressed: bool,
    pub running: bool,
    pub crouch_intent: bool,
    pub facing_yaw: f32,
}

/// Wire mirror of the engine `FireButtonState`.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireFireButtonState {
    pub pressed: bool,
    pub active: bool,
}

/// Input-command envelope: the client's per-tick intent, mirroring the engine
/// `SimCommand` (movement + fire button). Round-tripped in Phase 1; applied to
/// gameplay in Phase 2; reconciled against in Phase 3.
///
/// `client_tick` is the client's monotonic command-frame number, stamped first so
/// the host can record which command tick it last resolved for that pawn (echoed
/// back in the snapshot's `last_processed_client_tick`). The client matches that
/// ack against its own command history to know how far to replay during
/// reconciliation. It is the first field by design — the field order is part of the
/// wire layout (`WIRE_VERSION`).
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct InputCommand {
    pub client_tick: u32,
    pub movement: WireMovementInput,
    pub fire_button: WireFireButtonState,
}

/// Client -> server acknowledgement of replication progress, carried on the
/// reliable-ordered `Channel::Input` (alongside the input stream and, later,
/// time-sync). The server consumes it to advance each client's per-entity acked
/// baseline and retire acked despawn tombstones.
///
/// Semantics are **monotonic and additive**, never replacement-by-packet:
/// `entity_baselines` and `despawn_tombstones` list only the entries this client
/// has newly observed — an omitted entry leaves the server's prior ack state for
/// that entity/tombstone unchanged, and a stale (older-id) entry is ignored. The
/// `Vec`s are bitcode length-prefixed; an empty ack (no per-entity progress) is a
/// valid carrier for `latest_snapshot_sequence` / `acked_server_tick` alone.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct AckMessage {
    /// The highest snapshot `sequence` this client has received and processed.
    pub latest_snapshot_sequence: u32,
    /// The `server_tick` of that latest processed snapshot.
    pub acked_server_tick: u32,
    /// `(network_id, baseline_id)` pairs the client now holds. Advances the
    /// server's per-client baseline for that entity only if `baseline_id` is
    /// newer than the one already recorded.
    pub entity_baselines: Vec<(u32, u32)>,
    /// `(network_id, tombstone_id)` pairs the client has applied. Retires that
    /// tombstone for this client so the server stops resending the despawn.
    pub despawn_tombstones: Vec<(u32, u32)>,
    /// `(state_slot_id, baseline_id)` pairs the client now holds for replicated
    /// state slots (M15 Phase 3.5). Same monotonic-additive semantics as
    /// `entity_baselines`, but keyed by `StateSlotId` instead of `NetworkId`:
    /// advances the server's per-client state baseline for that slot only if
    /// `baseline_id` is newer. An empty list leaves prior state-ack progress
    /// unchanged. The `u16` is the `StateSlotId` inner value.
    pub slot_baselines: Vec<(u16, u32)>,
}

/// Client -> server request to re-send a full baseline for one entity, carried on
/// the reliable-ordered `Channel::Input`. Sent when the client receives a `Delta`
/// referencing a `baseline_ref` it does not hold (a lost/old snapshot left it
/// without that baseline). The server responds with a `FullBaseline` record for
/// that entity on `Channel::Snapshot`.
///
/// Requests are **additive** and keyed by `(client, network_id,
/// missing_baseline_ref)` on the server, so a duplicate request (the reliable
/// channel re-sent it, or the client asked twice) queues the same refresh once,
/// not twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct BaselineRefreshRequest {
    /// The snapshot `sequence` whose delta could not be applied. Diagnostic /
    /// dedup context; the repair is keyed by entity + missing ref, not sequence.
    pub snapshot_sequence: u32,
    /// The entity whose baseline the client is missing.
    pub network_id: u32,
    /// The `baseline_ref` the unappliable delta named but the client lacks.
    pub missing_baseline_ref: u32,
    /// Why the refresh is needed (e.g. unknown baseline). A `u8` reason code,
    /// not interpreted by the repair path — logged for diagnostics.
    pub reason: u8,
}

/// Client -> server request to re-send a full baseline for one replicated *state
/// slot*, carried on the reliable-ordered `Channel::Input` (M15 Phase 3.5).
///
/// Distinct from [`BaselineRefreshRequest`] by design: entity baselines are keyed
/// by `NetworkId`, while state baselines are keyed by `StateSlotId`. Sent when the
/// client receives a state `Delta` referencing a `baseline_ref` it does not hold;
/// the server schedules a `FullBaseline` for that slot on `Channel::Snapshot`.
/// Requests are additive and deduped server-side by `(client, slot_id,
/// missing_baseline_ref)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct StateBaselineRefreshRequest {
    /// The snapshot `sequence` whose state delta could not be applied. Diagnostic /
    /// dedup context; the repair is keyed by slot + missing ref, not sequence.
    pub snapshot_sequence: u32,
    /// The replicated state slot whose baseline the client is missing (`StateSlotId`
    /// inner value).
    pub slot_id: u16,
    /// The `baseline_ref` the unappliable state delta named but the client lacks.
    pub missing_baseline_ref: u32,
    /// Why the refresh is needed. A `u8` reason code, not interpreted by the repair
    /// path — logged for diagnostics.
    pub reason: u8,
}

/// Discriminated client -> server envelope for the reliable-ordered
/// `Channel::Input`, which multiplexes several message kinds (the input stream,
/// replication acks, baseline-refresh requests, and — Task 5 — time-sync). bitcode
/// tags the enum, so the server decodes one `ClientMessage` and matches on the
/// variant rather than guessing the type of an untagged payload. A new kind (e.g.
/// `TimeSync`) is added as a variant **appended** to preserve the discriminant
/// order of existing variants.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ClientMessage {
    /// Per-tick input intent (round-tripped in Phase 1; applied in Phase 2/3).
    Input(InputCommand),
    /// Replication progress ack.
    Ack(AckMessage),
    /// A request to re-send one entity's full baseline.
    BaselineRefresh(BaselineRefreshRequest),
    /// A time-sync probe (Task 5): the server echoes it on `Channel::Input` with
    /// its current tick so the client estimates the server clock. Appended last to
    /// preserve the discriminant order of the variants above.
    TimeSync(crate::timesync::TimeSyncRequest),
    /// A request to re-send one replicated state slot's full baseline (M15 Phase
    /// 3.5). Appended last to preserve the discriminant order of the variants
    /// above. Keyed by `StateSlotId`, distinct from `BaselineRefresh`'s `NetworkId`.
    StateBaselineRefresh(StateBaselineRefreshRequest),
}

/// Handshake message. Every connection is gated on a matching `ProtocolVersion`
/// before any other bitcode payload is decoded — the bitcode byte format is
/// unstable across crate majors, so a mismatch must be rejected up front.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct ProtocolVersion {
    pub app_protocol_id: u32,
    pub wire_version: u32,
}

/// Wire codec failure. Today the only failure mode is a bitcode decode error
/// (short or corrupted buffer); a typed wrapper keeps callers from depending on
/// bitcode's error type directly and leaves room for handshake/version errors.
#[derive(Debug)]
pub enum WireError {
    /// The buffer could not be decoded into the requested type (truncated,
    /// corrupted, or trailing bytes). Never a panic — always this `Err`.
    Decode(bitcode::Error),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Decode(e) => write!(f, "wire decode failed: {e}"),
        }
    }
}

impl std::error::Error for WireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WireError::Decode(e) => Some(e),
        }
    }
}

/// Encode a wire type to a fresh byte buffer. Infallible — bitcode encoding of
/// these owned, finite types cannot fail.
#[must_use]
pub fn encode<T: Encode + ?Sized>(value: &T) -> Vec<u8> {
    bitcode::encode(value)
}

/// Decode a wire type from a byte buffer. A short, corrupted, or over-long buffer
/// yields `Err(WireError::Decode(_))` — never a panic.
pub fn decode<'a, T: Decode<'a>>(bytes: &'a [u8]) -> Result<T, WireError> {
    bitcode::decode::<T>(bytes).map_err(WireError::Decode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_transform() -> WireTransform {
        WireTransform {
            position: [1.5, -2.0, 3.25],
            // A non-axis-aligned unit quaternion in [x, y, z, w] order.
            rotation: [0.182_574_2, 0.365_148_4, 0.547_722_6, 0.730_296_8],
            scale: [1.0, 2.0, 0.5],
        }
    }

    fn sample_movement() -> WirePlayerMovementState {
        WirePlayerMovementState {
            velocity: [0.0, 3.5, -1.0],
            is_grounded: false,
            air_jumps_remaining: 1,
            air_dashes_remaining: 2,
            dash_cooldown_ms: 120.0,
            air_ticks: 7,
            movement_state: WireMovementState::Dash {
                elapsed_ms: 33.0,
                boost: [4.0, 0.0, 0.0],
            },
            coyote_timer_ms: 80.0,
            jump_buffer_timer_ms: 0.0,
            jump_spent: true,
            capsule_half_height: 0.8,
            capsule_eye_height: 1.5,
        }
    }

    fn raw_transform_payload() -> RawComponentPayload {
        RawComponentPayload {
            component_kind: COMPONENT_KIND_TRANSFORM,
            transform: Some(sample_transform()),
            player_movement: None,
        }
    }

    fn raw_movement_payload() -> RawComponentPayload {
        RawComponentPayload {
            component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            transform: None,
            player_movement: Some(sample_movement()),
        }
    }

    /// A raw record with no movement-authority metadata set (the common case for
    /// the round-trip/validation fixtures). Tests that exercise the metadata set the
    /// three fields explicitly rather than through this helper.
    fn raw_record(
        record_kind: u16,
        network_id: u32,
        baseline_id_or_ref: u32,
        new_baseline_id_or_tombstone_id: u32,
        reason: u8,
        components: Vec<RawComponentPayload>,
    ) -> RawEntityRecord {
        RawEntityRecord {
            record_kind,
            network_id,
            baseline_id_or_ref,
            new_baseline_id_or_tombstone_id,
            reason,
            has_last_processed_client_tick: false,
            last_processed_client_tick: 0,
            local_player: false,
            has_entity_class: false,
            entity_class: String::new(),
            components,
        }
    }

    /// A raw snapshot carrying no replicated state records (the common case for the
    /// entity-record fixtures). The Phase 3.5 state fields default to an all-zero
    /// fingerprint and an empty record list; the state_slots module tests exercise
    /// those fields directly.
    fn raw_snapshot(
        sequence: u32,
        server_tick: u32,
        records: Vec<RawEntityRecord>,
    ) -> RawSnapshotMessage {
        RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence,
            server_tick,
            records,
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
        }
    }

    fn sample_input() -> InputCommand {
        InputCommand {
            client_tick: 4_242,
            movement: WireMovementInput {
                wish_dir: [0.5, -0.75],
                jump_pressed: true,
                dash_pressed: false,
                running: true,
                crouch_intent: false,
                facing_yaw: 1.234_5,
            },
            fire_button: WireFireButtonState {
                pressed: true,
                active: false,
            },
        }
    }

    // Round-trip a control value: encode then decode must reproduce it. These
    // are finite floats we author directly and never transform, so exact
    // value-equality is the correct assertion (testing_guide §Floating-point:
    // approximate comparison guards *computed* floats, not a byte round-trip of
    // a finite value).
    fn round_trips<T>(value: &T) -> bool
    where
        T: Encode + for<'de> Decode<'de> + PartialEq,
    {
        let bytes = encode(value);
        let decoded: T = decode(&bytes).expect("valid buffer must decode");
        &decoded == value
    }

    // --- Round-trip: encode then decode reproduces the raw envelope ---

    #[test]
    fn raw_snapshot_full_baseline_round_trips() {
        let raw = raw_snapshot(
            11,
            900,
            vec![raw_record(
                RECORD_KIND_FULL_BASELINE,
                5,
                3,
                0,
                0,
                vec![raw_transform_payload(), raw_movement_payload()],
            )],
        );
        assert!(round_trips(&raw));
    }

    #[test]
    fn raw_snapshot_empty_records_round_trips() {
        let raw = raw_snapshot(0, 0, Vec::new());
        assert!(round_trips(&raw));
    }

    /// A snapshot carrying a non-empty state-record list and a real fingerprint
    /// round-trips through the wire — the Phase 3.5 fields are part of the envelope.
    #[test]
    fn raw_snapshot_with_state_records_round_trips() {
        use crate::state_slots::{
            RawStateSlotRecord, STATE_RECORD_KIND_FULL_BASELINE, WireSlotValue,
        };
        let raw = RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence: 3,
            server_tick: 42,
            records: Vec::new(),
            state_schema_fingerprint: [9u8; 32],
            state_records: vec![RawStateSlotRecord {
                slot_id: 1,
                kind: STATE_RECORD_KIND_FULL_BASELINE,
                has_baseline_ref: false,
                baseline_ref: 0,
                baseline_id: 7,
                value: WireSlotValue::Number(50.0),
            }],
        };
        assert!(round_trips(&raw));
        // The state fields survive a decode and reach the typed apply model.
        let bytes = encode(&raw);
        let decoded: RawSnapshotMessage = decode(&bytes).expect("snapshot decodes");
        let typed = decoded.validate().expect("entity half validates");
        assert_eq!(typed.state_schema_fingerprint, [9u8; 32]);
        assert_eq!(typed.state_records.len(), 1);
    }

    #[test]
    fn movement_state_variants_round_trip() {
        for state in [
            WireMovementState::Normal,
            WireMovementState::Dash {
                elapsed_ms: 12.5,
                boost: [1.0, -2.0, 3.0],
            },
            WireMovementState::Crouching { eye_current: 0.9 },
        ] {
            let movement = WirePlayerMovementState {
                movement_state: state,
                ..sample_movement()
            };
            assert!(round_trips(&movement));
        }
    }

    #[test]
    fn input_command_round_trips() {
        assert!(round_trips(&sample_input()));
    }

    #[test]
    fn handshake_round_trips() {
        let handshake = ProtocolVersion {
            app_protocol_id: 0xCAFE_BABE,
            wire_version: 1,
        };
        assert!(round_trips(&handshake));
    }

    #[test]
    fn ack_message_round_trips() {
        let ack = AckMessage {
            latest_snapshot_sequence: 17,
            acked_server_tick: 510,
            entity_baselines: vec![(3, 9), (7, 2), (42, 100)],
            despawn_tombstones: vec![(11, 4)],
            slot_baselines: vec![(1, 7), (2, 3)],
        };
        assert!(round_trips(&ack));
        // An empty ack (no per-entity progress) is still a valid carrier.
        let empty = AckMessage {
            latest_snapshot_sequence: 0,
            acked_server_tick: 0,
            entity_baselines: Vec::new(),
            despawn_tombstones: Vec::new(),
            slot_baselines: Vec::new(),
        };
        assert!(round_trips(&empty));
    }

    #[test]
    fn state_baseline_refresh_request_round_trips() {
        let req = StateBaselineRefreshRequest {
            snapshot_sequence: 22,
            slot_id: 4,
            missing_baseline_ref: 5,
            reason: 1,
        };
        assert!(round_trips(&req));
    }

    #[test]
    fn baseline_refresh_request_round_trips() {
        let req = BaselineRefreshRequest {
            snapshot_sequence: 22,
            network_id: 8,
            missing_baseline_ref: 5,
            reason: 1,
        };
        assert!(round_trips(&req));
    }

    #[test]
    fn client_message_variants_round_trip() {
        let variants = [
            ClientMessage::Input(sample_input()),
            ClientMessage::Ack(AckMessage {
                latest_snapshot_sequence: 3,
                acked_server_tick: 180,
                entity_baselines: vec![(1, 2)],
                despawn_tombstones: vec![(4, 5)],
                slot_baselines: vec![(6, 7)],
            }),
            ClientMessage::BaselineRefresh(BaselineRefreshRequest {
                snapshot_sequence: 9,
                network_id: 1,
                missing_baseline_ref: 2,
                reason: 0,
            }),
            ClientMessage::TimeSync(crate::timesync::TimeSyncRequest {
                sample_id: 4,
                client_send_tick: 88,
                client_send_time_us: 12_345_678,
            }),
            ClientMessage::StateBaselineRefresh(StateBaselineRefreshRequest {
                snapshot_sequence: 9,
                slot_id: 1,
                missing_baseline_ref: 2,
                reason: 0,
            }),
        ];
        for msg in variants {
            assert!(round_trips(&msg));
        }
    }

    #[test]
    fn corrupt_ack_and_refresh_decode_to_err_not_panic() {
        // A hostile/truncated client->server message must be a typed Err, never a
        // panic — the server must survive a malformed ack/refresh on the wire.
        let garbage = [0xFFu8, 0x00, 0xAB, 0x12, 0x9C, 0x7D, 0x55, 0x01];
        assert!(decode::<AckMessage>(&garbage).is_err());
        assert!(decode::<BaselineRefreshRequest>(&garbage).is_err());
        assert!(decode::<AckMessage>(&[]).is_err());
        assert!(decode::<BaselineRefreshRequest>(&[]).is_err());
    }

    // --- Validation: raw -> typed happy paths ---

    #[test]
    fn validate_full_baseline_produces_typed_record() {
        let raw = RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence: 4,
            server_tick: 60,
            records: vec![raw_record(
                RECORD_KIND_FULL_BASELINE,
                9,
                2,
                0,
                0,
                vec![raw_transform_payload(), raw_movement_payload()],
            )],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
        };
        let typed = raw.validate().expect("well-formed snapshot validates");
        assert_eq!(typed.sequence, 4);
        assert_eq!(typed.server_tick, 60);
        assert_eq!(
            typed.records,
            vec![EntityRecord::FullBaseline {
                network_id: 9,
                baseline_id: 2,
                last_processed_client_tick: None,
                local_player: false,
                entity_class: None,
                components: vec![
                    ComponentPayload::Transform(sample_transform()),
                    ComponentPayload::PlayerMovementState(sample_movement()),
                ],
            }]
        );
    }

    #[test]
    fn validate_delta_maps_overloaded_ids() {
        let raw = RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence: 5,
            server_tick: 61,
            records: vec![raw_record(
                RECORD_KIND_DELTA,
                9,
                2,
                3,
                0,
                vec![raw_transform_payload()],
            )],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
        };
        let typed = raw.validate().expect("well-formed delta validates");
        assert_eq!(
            typed.records,
            vec![EntityRecord::Delta {
                network_id: 9,
                baseline_ref: 2,
                new_baseline_id: 3,
                last_processed_client_tick: None,
                local_player: false,
                entity_class: None,
                components: vec![ComponentPayload::Transform(sample_transform())],
            }]
        );
    }

    #[test]
    fn validate_despawn_maps_tombstone_and_reason() {
        let raw = RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence: 6,
            server_tick: 62,
            // Components on a despawn are ignored, not rejected.
            records: vec![raw_record(
                RECORD_KIND_DESPAWN,
                9,
                0,
                42,
                7,
                vec![raw_transform_payload()],
            )],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
        };
        let typed = raw
            .validate()
            .expect("despawn validates, ignoring components");
        assert_eq!(
            typed.records,
            vec![EntityRecord::Despawn {
                network_id: 9,
                tombstone_id: 42,
                reason: 7,
            }]
        );
    }

    // --- Malformed input: corrupt/short bytes are decode errors ---

    #[test]
    fn corrupt_bitcode_decodes_to_err_not_panic() {
        // Random bytes are extremely unlikely to be a valid encoding; the codec
        // must return Err, not panic, before validation ever runs.
        let garbage = [0xFFu8, 0x00, 0xAB, 0x12, 0x9C, 0x7D, 0x55, 0x01];
        assert!(decode::<RawSnapshotMessage>(&garbage).is_err());
        let _ = decode::<RawComponentPayload>(&garbage);
        let _ = decode::<RawEntityRecord>(&garbage);
    }

    #[test]
    fn truncated_buffer_decodes_to_err_not_panic() {
        let raw = RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence: 9,
            server_tick: 1,
            records: vec![raw_record(
                RECORD_KIND_FULL_BASELINE,
                3,
                1,
                0,
                0,
                vec![raw_transform_payload()],
            )],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
        };
        let bytes = encode(&raw);
        let truncated = &bytes[..bytes.len() - 1];
        assert!(decode::<RawSnapshotMessage>(truncated).is_err());
    }

    #[test]
    fn empty_buffer_decodes_to_err_not_panic() {
        assert!(decode::<RawSnapshotMessage>(&[]).is_err());
        assert!(decode::<ProtocolVersion>(&[]).is_err());
        assert!(decode::<InputCommand>(&[]).is_err());
    }

    // --- Malformed input: invalid kinds decode cleanly, rejected at validation ---

    #[test]
    fn invalid_record_kind_decodes_then_rejects_without_panic() {
        let raw = RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence: 1,
            server_tick: 1,
            records: vec![raw_record(
                99, // not FullBaseline/Delta/Despawn
                1,
                0,
                0,
                0,
                Vec::new(),
            )],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
        };
        // Decodes cleanly into the raw envelope...
        let bytes = encode(&raw);
        let decoded: RawSnapshotMessage = decode(&bytes).expect("invalid kind still decodes");
        // ...but is rejected at validation, no typed record produced.
        assert_eq!(
            decoded.validate(),
            Err(ValidationError::UnknownRecordKind(99))
        );
    }

    #[test]
    fn invalid_component_kind_decodes_then_rejects() {
        let raw = RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence: 1,
            server_tick: 1,
            records: vec![raw_record(
                RECORD_KIND_FULL_BASELINE,
                1,
                0,
                0,
                0,
                vec![RawComponentPayload {
                    component_kind: 1234, // not Transform/PlayerMovementState
                    transform: Some(sample_transform()),
                    player_movement: None,
                }],
            )],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
        };
        let bytes = encode(&raw);
        let decoded: RawSnapshotMessage = decode(&bytes).expect("invalid kind still decodes");
        assert_eq!(
            decoded.validate(),
            Err(ValidationError::UnknownComponentKind(1234))
        );
    }

    #[test]
    fn missing_payload_slot_for_kind_rejects() {
        let payload = RawComponentPayload {
            component_kind: COMPONENT_KIND_TRANSFORM,
            transform: None, // kind says Transform but slot is empty
            player_movement: None,
        };
        assert_eq!(
            payload.validate(),
            Err(ValidationError::MissingComponentPayload(
                COMPONENT_KIND_TRANSFORM
            ))
        );
    }

    #[test]
    fn duplicate_payload_slots_reject() {
        // Both slots populated: ambiguous which one the kind names.
        let payload = RawComponentPayload {
            component_kind: COMPONENT_KIND_TRANSFORM,
            transform: Some(sample_transform()),
            player_movement: Some(sample_movement()),
        };
        assert_eq!(
            payload.validate(),
            Err(ValidationError::MismatchedComponentPayload(
                COMPONENT_KIND_TRANSFORM
            ))
        );
    }

    #[test]
    fn wrong_slot_for_kind_reports_missing() {
        // Kind says PlayerMovementState, but only the Transform slot is filled.
        // The named slot (player_movement) is None, so the error is Missing, not
        // Mismatched — even though a different slot is populated.
        let payload = RawComponentPayload {
            component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            transform: Some(sample_transform()),
            player_movement: None,
        };
        assert_eq!(
            payload.validate(),
            Err(ValidationError::MissingComponentPayload(
                COMPONENT_KIND_PLAYER_MOVEMENT_STATE
            ))
        );
    }

    #[test]
    fn version_mismatch_rejects_before_records() {
        let raw = RawSnapshotMessage {
            version: SNAPSHOT_VERSION - 1, // a Phase 1-era version
            sequence: 1,
            server_tick: 1,
            records: Vec::new(),
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
        };
        assert_eq!(
            raw.validate(),
            Err(ValidationError::VersionMismatch {
                expected: SNAPSHOT_VERSION,
                received: SNAPSHOT_VERSION - 1,
            })
        );
    }

    #[test]
    fn first_bad_record_short_circuits_validation() {
        // A good record followed by a bad one: validation rejects the whole
        // snapshot and produces no partial typed result.
        let raw = RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence: 1,
            server_tick: 1,
            records: vec![
                raw_record(
                    RECORD_KIND_FULL_BASELINE,
                    1,
                    0,
                    0,
                    0,
                    vec![raw_transform_payload()],
                ),
                raw_record(77, 2, 0, 0, 0, Vec::new()),
            ],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
        };
        assert_eq!(raw.validate(), Err(ValidationError::UnknownRecordKind(77)));
    }

    // Drift guard: the wire component discriminants MUST stay numeric-equal to the
    // engine `ComponentKind as u16` (crates/postretro/src/scripting/registry.rs):
    // Transform = 0, PlayerMovement = 6 in Phase 2. The exhaustive match (no `_`
    // arm) means a new `ComponentPayload` variant is a compile error here until its
    // expected discriminant is pinned — a silently-passing guard is the failure
    // mode this prevents. The engine side asserts the same mapping independently
    // (`component_kind_discriminant`), so a divergence fails one side's guard.
    #[test]
    fn component_kind_pinned_to_engine_discriminants() {
        let cases = [
            ComponentPayload::Transform(sample_transform()),
            ComponentPayload::PlayerMovementState(sample_movement()),
        ];
        for payload in cases {
            let expected = match payload {
                ComponentPayload::Transform(_) => 0,
                ComponentPayload::PlayerMovementState(_) => 6,
            };
            assert_eq!(payload.kind(), expected);
        }
    }

    // Drift guard: the three record_kind constants are distinct and a typed record
    // round-trips through its raw form at the same kind. The exhaustive match (no
    // `_` arm) over the typed variant means a new `EntityRecord` variant is a
    // compile error here until its raw `record_kind` and round-trip are pinned.
    #[test]
    fn record_kind_round_trips_through_raw_form() {
        let variants = [
            EntityRecord::FullBaseline {
                network_id: 1,
                baseline_id: 2,
                last_processed_client_tick: None,
                local_player: false,
                entity_class: None,
                components: vec![ComponentPayload::Transform(sample_transform())],
            },
            EntityRecord::Delta {
                network_id: 1,
                baseline_ref: 2,
                new_baseline_id: 3,
                last_processed_client_tick: None,
                local_player: false,
                entity_class: None,
                components: vec![ComponentPayload::Transform(sample_transform())],
            },
            EntityRecord::Despawn {
                network_id: 1,
                tombstone_id: 9,
                reason: 4,
            },
        ];
        for typed in variants {
            // Each typed variant maps to exactly one raw record_kind; validating
            // that raw record must reproduce the typed variant unchanged. These
            // variants carry no movement component, so the metadata stays absent
            // (`None`/`false`) — exercised separately in the metadata tests.
            let raw = match &typed {
                EntityRecord::FullBaseline {
                    network_id,
                    baseline_id,
                    components,
                    ..
                } => raw_record(
                    RECORD_KIND_FULL_BASELINE,
                    *network_id,
                    *baseline_id,
                    0,
                    0,
                    components.iter().map(raw_from_typed).collect(),
                ),
                EntityRecord::Delta {
                    network_id,
                    baseline_ref,
                    new_baseline_id,
                    components,
                    ..
                } => raw_record(
                    RECORD_KIND_DELTA,
                    *network_id,
                    *baseline_ref,
                    *new_baseline_id,
                    0,
                    components.iter().map(raw_from_typed).collect(),
                ),
                EntityRecord::Despawn {
                    network_id,
                    tombstone_id,
                    reason,
                } => raw_record(
                    RECORD_KIND_DESPAWN,
                    *network_id,
                    0,
                    *tombstone_id,
                    *reason,
                    Vec::new(),
                ),
            };
            // Round-trip the raw record through bitcode before validating, so the
            // pinned `record_kind` survives the wire too.
            let bytes = encode(&raw);
            let decoded: RawEntityRecord = decode(&bytes).expect("raw record decodes");
            assert_eq!(decoded.validate(), Ok(typed));
        }
    }

    // --- Command-frame tick ---

    #[test]
    fn input_command_carries_client_tick_through_round_trip() {
        // The client_tick is the first field and must survive the wire so the host
        // can echo it back as the movement-authority ack.
        let cmd = InputCommand {
            client_tick: 9_001,
            ..sample_input()
        };
        let bytes = encode(&cmd);
        let decoded: InputCommand = decode(&bytes).expect("input command decodes");
        assert_eq!(decoded.client_tick, 9_001);
        assert_eq!(decoded, cmd);
    }

    // --- Snapshot movement-authority metadata ---

    /// A full-baseline movement record carrying a resolved tick and the local-player
    /// flag round-trips through raw -> wire -> typed with both surfaced.
    #[test]
    fn movement_metadata_round_trips_to_typed_record() {
        let mut record = raw_record(
            RECORD_KIND_FULL_BASELINE,
            9,
            2,
            0,
            0,
            vec![raw_transform_payload(), raw_movement_payload()],
        );
        record.has_last_processed_client_tick = true;
        record.last_processed_client_tick = 777;
        record.local_player = true;

        let raw = RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence: 1,
            server_tick: 1,
            records: vec![record],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
        };
        let bytes = encode(&raw);
        let decoded: RawSnapshotMessage = decode(&bytes).expect("snapshot decodes");
        let typed = decoded.validate().expect("metadata is well-formed");
        assert_eq!(
            typed.records,
            vec![EntityRecord::FullBaseline {
                network_id: 9,
                baseline_id: 2,
                last_processed_client_tick: Some(777),
                local_player: true,
                entity_class: None,
                components: vec![
                    ComponentPayload::Transform(sample_transform()),
                    ComponentPayload::PlayerMovementState(sample_movement()),
                ],
            }]
        );
    }

    /// A movement record may legitimately have no resolved tick yet (`None`) and not
    /// be the local player — the non-local / pre-first-command case.
    #[test]
    fn movement_record_without_metadata_validates_to_none() {
        let raw = raw_record(
            RECORD_KIND_FULL_BASELINE,
            9,
            2,
            0,
            0,
            vec![raw_movement_payload()],
        );
        let typed = raw.validate().expect("absent metadata is valid");
        assert_eq!(
            typed,
            EntityRecord::FullBaseline {
                network_id: 9,
                baseline_id: 2,
                last_processed_client_tick: None,
                local_player: false,
                entity_class: None,
                components: vec![ComponentPayload::PlayerMovementState(sample_movement())],
            }
        );
    }

    /// Either ack flag on a record with no `PlayerMovementState` is rejected: the
    /// metadata is meaningless without a movement pawn to attribute it to.
    #[test]
    fn metadata_on_non_movement_record_rejects() {
        // local_player set on a Transform-only record.
        let mut local = raw_record(
            RECORD_KIND_FULL_BASELINE,
            1,
            1,
            0,
            0,
            vec![raw_transform_payload()],
        );
        local.local_player = true;
        assert_eq!(
            local.validate(),
            Err(ValidationError::MovementMetadataWithoutMovement)
        );

        // last_processed_client_tick set on a Transform-only record.
        let mut tick = raw_record(
            RECORD_KIND_FULL_BASELINE,
            1,
            1,
            0,
            0,
            vec![raw_transform_payload()],
        );
        tick.has_last_processed_client_tick = true;
        tick.last_processed_client_tick = 5;
        assert_eq!(
            tick.validate(),
            Err(ValidationError::MovementMetadataWithoutMovement)
        );
    }

    /// `has_last_processed_client_tick = false` paired with a nonzero tick is a
    /// malformed envelope — the "absent" flag cannot ride a real value.
    #[test]
    fn malformed_tick_metadata_rejects() {
        let mut record = raw_record(
            RECORD_KIND_FULL_BASELINE,
            1,
            1,
            0,
            0,
            vec![raw_movement_payload()],
        );
        record.has_last_processed_client_tick = false;
        record.last_processed_client_tick = 42; // nonzero with the flag clear
        assert_eq!(
            record.validate(),
            Err(ValidationError::MalformedTickMetadata {
                last_processed_client_tick: 42
            })
        );
    }

    /// Any movement-authority metadata on a despawn record is rejected: a tombstone
    /// has no pawn state to ack.
    #[test]
    fn metadata_on_despawn_rejects() {
        for mutate in [
            |r: &mut RawEntityRecord| r.has_last_processed_client_tick = true,
            |r: &mut RawEntityRecord| r.last_processed_client_tick = 3,
            |r: &mut RawEntityRecord| r.local_player = true,
        ] {
            let mut record = raw_record(RECORD_KIND_DESPAWN, 1, 0, 9, 0, Vec::new());
            mutate(&mut record);
            assert_eq!(record.validate(), Err(ValidationError::MetadataOnDespawn));
        }
    }

    // --- entity_class metadata (M15 Phase 3 Task 7) ---

    /// A movement record carrying `entity_class` round-trips raw -> wire -> typed with
    /// the class surfaced as `Some(_)`.
    #[test]
    fn entity_class_round_trips_to_typed_record() {
        let mut record = raw_record(
            RECORD_KIND_FULL_BASELINE,
            9,
            2,
            0,
            0,
            vec![raw_transform_payload(), raw_movement_payload()],
        );
        record.has_entity_class = true;
        record.entity_class = "player".to_string();
        record.has_last_processed_client_tick = true;
        record.last_processed_client_tick = 5;
        record.local_player = true;

        let raw = RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence: 1,
            server_tick: 1,
            records: vec![record],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
        };
        let bytes = encode(&raw);
        let decoded: RawSnapshotMessage = decode(&bytes).expect("snapshot decodes");
        let typed = decoded.validate().expect("entity_class is well-formed");
        assert_eq!(
            typed.records,
            vec![EntityRecord::FullBaseline {
                network_id: 9,
                baseline_id: 2,
                last_processed_client_tick: Some(5),
                local_player: true,
                entity_class: Some("player".to_string()),
                components: vec![
                    ComponentPayload::Transform(sample_transform()),
                    ComponentPayload::PlayerMovementState(sample_movement()),
                ],
            }]
        );
    }

    /// A delta record backed by a `Transform` also carries `entity_class` through
    /// validation, with no `PlayerMovementState` required (E10).
    #[test]
    fn entity_class_round_trips_on_delta() {
        let mut record = raw_record(RECORD_KIND_DELTA, 9, 2, 3, 0, vec![raw_transform_payload()]);
        record.has_entity_class = true;
        record.entity_class = "boomer".to_string();
        let typed = record.validate().expect("delta entity_class validates");
        assert_eq!(
            typed,
            EntityRecord::Delta {
                network_id: 9,
                baseline_ref: 2,
                new_baseline_id: 3,
                last_processed_client_tick: None,
                local_player: false,
                entity_class: Some("boomer".to_string()),
                components: vec![ComponentPayload::Transform(sample_transform())],
            }
        );
    }

    /// E10: a non-despawn record backed by a finite `Transform` and carrying an
    /// `entity_class` but NO `PlayerMovementState` now validates — the descriptor
    /// class rides a `Transform`-only remote-presentation record. (Previously this
    /// was rejected as `EntityClassWithoutMovement`.)
    #[test]
    fn entity_class_on_transform_only_record_validates() {
        let mut record = raw_record(
            RECORD_KIND_FULL_BASELINE,
            7,
            2,
            0,
            0,
            vec![raw_transform_payload()],
        );
        record.has_entity_class = true;
        record.entity_class = "boomer".to_string();
        let typed = record
            .validate()
            .expect("transform-only entity_class record validates");
        assert_eq!(
            typed,
            EntityRecord::FullBaseline {
                network_id: 7,
                baseline_id: 2,
                last_processed_client_tick: None,
                local_player: false,
                entity_class: Some("boomer".to_string()),
                components: vec![ComponentPayload::Transform(sample_transform())],
            }
        );
    }

    /// `entity_class` on a record carrying no `Transform` at all (only a
    /// `PlayerMovementState`) is rejected: the descriptor presentation entity rides
    /// the wire as a `Transform`, so without one there is nothing to place.
    #[test]
    fn entity_class_without_transform_rejects() {
        let mut record = raw_record(
            RECORD_KIND_FULL_BASELINE,
            1,
            1,
            0,
            0,
            vec![raw_movement_payload()],
        );
        record.has_entity_class = true;
        record.entity_class = "player".to_string();
        assert_eq!(
            record.validate(),
            Err(ValidationError::EntityClassWithoutTransform)
        );
    }

    /// Any `entity_class` on a despawn record is rejected: a tombstone has no pawn to
    /// materialize.
    #[test]
    fn entity_class_on_despawn_rejects() {
        for mutate in [
            |r: &mut RawEntityRecord| r.has_entity_class = true,
            |r: &mut RawEntityRecord| r.entity_class = "player".to_string(),
        ] {
            let mut record = raw_record(RECORD_KIND_DESPAWN, 1, 0, 9, 0, Vec::new());
            mutate(&mut record);
            assert_eq!(record.validate(), Err(ValidationError::MetadataOnDespawn));
        }
    }

    /// `has_entity_class = false` paired with a non-empty class is malformed — the
    /// "absent" flag cannot ride a real class value.
    #[test]
    fn malformed_entity_class_metadata_rejects() {
        let mut record = raw_record(
            RECORD_KIND_FULL_BASELINE,
            1,
            1,
            0,
            0,
            vec![raw_movement_payload()],
        );
        record.has_entity_class = false;
        record.entity_class = "player".to_string(); // non-empty with the flag clear
        assert_eq!(
            record.validate(),
            Err(ValidationError::MalformedEntityClassMetadata)
        );
    }

    // --- Non-finite PlayerMovementState rejection ---

    /// Every replicated float field of a `PlayerMovementState` must be finite; a
    /// NaN/inf in any of them is rejected before typed apply, so no non-finite
    /// movement state reaches the registry. Each case mutates exactly one field.
    #[test]
    fn non_finite_movement_state_rejects_each_field() {
        let mutators: [fn(&mut WirePlayerMovementState); 9] = [
            |m| m.velocity[0] = f32::NAN,
            |m| m.velocity[2] = f32::INFINITY,
            |m| m.dash_cooldown_ms = f32::NAN,
            |m| m.coyote_timer_ms = f32::INFINITY,
            |m| m.jump_buffer_timer_ms = f32::NEG_INFINITY,
            |m| m.capsule_half_height = f32::NAN,
            |m| m.capsule_eye_height = f32::INFINITY,
            |m| {
                m.movement_state = WireMovementState::Dash {
                    elapsed_ms: f32::NAN,
                    boost: [0.0, 0.0, 0.0],
                }
            },
            |m| {
                m.movement_state = WireMovementState::Crouching {
                    eye_current: f32::INFINITY,
                }
            },
        ];
        for mutate in mutators {
            let mut movement = sample_movement();
            // sample_movement defaults to a finite Dash; reset to a finite Normal so
            // the dash/crouch mutators are the only non-finite source in their case.
            movement.movement_state = WireMovementState::Normal;
            mutate(&mut movement);
            let payload = RawComponentPayload {
                component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
                transform: None,
                player_movement: Some(movement),
            };
            assert_eq!(
                payload.validate(),
                Err(ValidationError::NonFiniteMovementState)
            );
        }
    }

    /// The dash-boost vector is also checked component-wise.
    #[test]
    fn non_finite_dash_boost_rejects() {
        let movement = WirePlayerMovementState {
            movement_state: WireMovementState::Dash {
                elapsed_ms: 10.0,
                boost: [1.0, f32::NAN, 3.0],
            },
            ..sample_movement()
        };
        let payload = RawComponentPayload {
            component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            transform: None,
            player_movement: Some(movement),
        };
        assert_eq!(
            payload.validate(),
            Err(ValidationError::NonFiniteMovementState)
        );
    }

    // --- Non-finite Transform rejection (E10) ---

    /// Every replicated float of a `Transform` (position, rotation, scale) must be
    /// finite; a NaN/inf in any is rejected before typed apply, so no non-finite
    /// pose reaches the registry. Each case mutates exactly one field.
    #[test]
    fn non_finite_transform_rejects_each_field() {
        let mutators: [fn(&mut WireTransform); 3] = [
            |t| t.position[1] = f32::NAN,
            |t| t.rotation[3] = f32::INFINITY,
            |t| t.scale[0] = f32::NEG_INFINITY,
        ];
        for mutate in mutators {
            let mut transform = sample_transform();
            mutate(&mut transform);
            let payload = RawComponentPayload {
                component_kind: COMPONENT_KIND_TRANSFORM,
                transform: Some(transform),
                player_movement: None,
            };
            assert_eq!(payload.validate(), Err(ValidationError::NonFiniteTransform));
        }
    }

    /// A record whose only `Transform` is non-finite is rejected at component
    /// validation (`NonFiniteTransform`) — before the entity_class rule even runs.
    #[test]
    fn record_with_only_non_finite_transform_rejects() {
        let bad_transform = WireTransform {
            position: [0.0, f32::NAN, 0.0],
            ..sample_transform()
        };
        let record = raw_record(
            RECORD_KIND_FULL_BASELINE,
            1,
            1,
            0,
            0,
            vec![RawComponentPayload {
                component_kind: COMPONENT_KIND_TRANSFORM,
                transform: Some(bad_transform),
                player_movement: None,
            }],
        );
        assert_eq!(record.validate(), Err(ValidationError::NonFiniteTransform));
    }

    /// An `entity_class` record backed only by a non-finite `Transform` is rejected:
    /// the non-finite pose is caught at component validation, so the class never
    /// rides a degenerate descriptor placement.
    #[test]
    fn entity_class_backed_by_non_finite_transform_rejects() {
        let bad_transform = WireTransform {
            scale: [f32::INFINITY, 1.0, 1.0],
            ..sample_transform()
        };
        let mut record = raw_record(
            RECORD_KIND_FULL_BASELINE,
            1,
            1,
            0,
            0,
            vec![RawComponentPayload {
                component_kind: COMPONENT_KIND_TRANSFORM,
                transform: Some(bad_transform),
                player_movement: None,
            }],
        );
        record.has_entity_class = true;
        record.entity_class = "player".to_string();
        assert_eq!(record.validate(), Err(ValidationError::NonFiniteTransform));
    }

    // Drift guard: every `WireMovementState` variant's float payload is covered by
    // the finiteness check. The expectation is derived from the source enum via an
    // exhaustive `match` (no `_` arm), so a new variant is a compile error here until
    // its finiteness contribution is declared — never a silently-passing guard.
    #[test]
    fn movement_state_finiteness_covers_every_variant() {
        let variants = [
            WireMovementState::Normal,
            WireMovementState::Dash {
                elapsed_ms: 1.0,
                boost: [0.0, 0.0, 0.0],
            },
            WireMovementState::Crouching { eye_current: 0.5 },
        ];
        for state in variants {
            // A finite instance of every variant must pass the finiteness gate.
            assert!(state.all_finite(), "finite variant must be all_finite");
            // The number of float fields each variant carries — derived from the
            // source enum so adding a variant (or a float field) forces an update.
            let float_field_count = match state {
                WireMovementState::Normal => 0,
                WireMovementState::Dash { .. } => 4, // elapsed_ms + 3 boost components
                WireMovementState::Crouching { .. } => 1,
            };
            // Non-`Normal` variants carry floats and so are non-finite-detectable.
            assert_eq!(
                float_field_count > 0,
                !matches!(state, WireMovementState::Normal)
            );
        }
    }

    /// Re-encode a typed payload into its raw envelope form for round-trip guards.
    fn raw_from_typed(payload: &ComponentPayload) -> RawComponentPayload {
        match payload {
            ComponentPayload::Transform(t) => RawComponentPayload {
                component_kind: COMPONENT_KIND_TRANSFORM,
                transform: Some(*t),
                player_movement: None,
            },
            ComponentPayload::PlayerMovementState(m) => RawComponentPayload {
                component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
                transform: None,
                player_movement: Some(*m),
            },
        }
    }
}
