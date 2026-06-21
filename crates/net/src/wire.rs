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
pub const SNAPSHOT_VERSION: u16 = 2;

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
        components: Vec<ComponentPayload>,
    },
    Delta {
        network_id: u32,
        baseline_ref: u32,
        new_baseline_id: u32,
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
    /// The payload slot required by `component_kind` was `None`.
    MissingComponentPayload(u16),
    /// More than one payload slot was `Some` (ambiguous which one `component_kind`
    /// names), or a slot was `Some` that does not match `component_kind`.
    MismatchedComponentPayload(u16),
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
                write!(f, "mismatched/duplicate payload slot for component_kind {k}")
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
        let populated = usize::from(self.transform.is_some())
            + usize::from(self.player_movement.is_some());

        match self.component_kind {
            COMPONENT_KIND_TRANSFORM => match self.transform {
                Some(t) if populated == 1 => Ok(ComponentPayload::Transform(t)),
                Some(_) => Err(ValidationError::MismatchedComponentPayload(self.component_kind)),
                None => Err(ValidationError::MissingComponentPayload(self.component_kind)),
            },
            COMPONENT_KIND_PLAYER_MOVEMENT_STATE => match self.player_movement {
                Some(m) if populated == 1 => Ok(ComponentPayload::PlayerMovementState(m)),
                Some(_) => Err(ValidationError::MismatchedComponentPayload(self.component_kind)),
                None => Err(ValidationError::MissingComponentPayload(self.component_kind)),
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
            RECORD_KIND_FULL_BASELINE => Ok(EntityRecord::FullBaseline {
                network_id: self.network_id,
                baseline_id: self.baseline_id_or_ref,
                components: self.validate_components()?,
            }),
            RECORD_KIND_DELTA => Ok(EntityRecord::Delta {
                network_id: self.network_id,
                baseline_ref: self.baseline_id_or_ref,
                new_baseline_id: self.new_baseline_id_or_tombstone_id,
                components: self.validate_components()?,
            }),
            // Despawn carries no components; any present are ignored, matching the
            // overloaded-field rule (only the fields a kind names are read).
            RECORD_KIND_DESPAWN => Ok(EntityRecord::Despawn {
                network_id: self.network_id,
                tombstone_id: self.new_baseline_id_or_tombstone_id,
                reason: self.reason,
            }),
            other => Err(ValidationError::UnknownRecordKind(other)),
        }
    }

    fn validate_components(&self) -> Result<Vec<ComponentPayload>, ValidationError> {
        self.components.iter().map(RawComponentPayload::validate).collect()
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
        })
    }
}

// ---------------------------------------------------------------------------
// Phase 1 full-state snapshot (still used by Phase 1 engine glue until Task 2/3
// rewires serialize/apply onto the lifecycle records above).
// ---------------------------------------------------------------------------

/// Full-state snapshot envelope: a server tick stamp plus a count-prefixed list
/// of `(NetworkId, ComponentPayload)`. bitcode length-prefixes the `Vec`, which
/// is the "count-prefixed entries" on the wire; an empty list encodes as count 0.
///
/// Phase 1 carrier — the Phase 2 wire is [`RawSnapshotMessage`]. `ComponentPayload`
/// is no longer `Encode`/`Decode` (it is the validated apply model), so this type's
/// codec derive is gone; it is constructed in-process by the engine glue, not sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub tick: u32,
    pub entries: Vec<(NetworkId, ComponentPayload)>,
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
/// gameplay in Phase 2.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct InputCommand {
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

    fn sample_input() -> InputCommand {
        InputCommand {
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
        let raw = RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence: 11,
            server_tick: 900,
            records: vec![RawEntityRecord {
                record_kind: RECORD_KIND_FULL_BASELINE,
                network_id: 5,
                baseline_id_or_ref: 3,
                new_baseline_id_or_tombstone_id: 0,
                reason: 0,
                components: vec![raw_transform_payload(), raw_movement_payload()],
            }],
        };
        assert!(round_trips(&raw));
    }

    #[test]
    fn raw_snapshot_empty_records_round_trips() {
        let raw = RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence: 0,
            server_tick: 0,
            records: Vec::new(),
        };
        assert!(round_trips(&raw));
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
        };
        assert!(round_trips(&ack));
        // An empty ack (no per-entity progress) is still a valid carrier.
        let empty = AckMessage {
            latest_snapshot_sequence: 0,
            acked_server_tick: 0,
            entity_baselines: Vec::new(),
            despawn_tombstones: Vec::new(),
        };
        assert!(round_trips(&empty));
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
            records: vec![RawEntityRecord {
                record_kind: RECORD_KIND_FULL_BASELINE,
                network_id: 9,
                baseline_id_or_ref: 2,
                new_baseline_id_or_tombstone_id: 0,
                reason: 0,
                components: vec![raw_transform_payload(), raw_movement_payload()],
            }],
        };
        let typed = raw.validate().expect("well-formed snapshot validates");
        assert_eq!(typed.sequence, 4);
        assert_eq!(typed.server_tick, 60);
        assert_eq!(
            typed.records,
            vec![EntityRecord::FullBaseline {
                network_id: 9,
                baseline_id: 2,
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
            records: vec![RawEntityRecord {
                record_kind: RECORD_KIND_DELTA,
                network_id: 9,
                baseline_id_or_ref: 2,
                new_baseline_id_or_tombstone_id: 3,
                reason: 0,
                components: vec![raw_transform_payload()],
            }],
        };
        let typed = raw.validate().expect("well-formed delta validates");
        assert_eq!(
            typed.records,
            vec![EntityRecord::Delta {
                network_id: 9,
                baseline_ref: 2,
                new_baseline_id: 3,
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
            records: vec![RawEntityRecord {
                record_kind: RECORD_KIND_DESPAWN,
                network_id: 9,
                baseline_id_or_ref: 0,
                new_baseline_id_or_tombstone_id: 42,
                reason: 7,
                // Components on a despawn are ignored, not rejected.
                components: vec![raw_transform_payload()],
            }],
        };
        let typed = raw.validate().expect("despawn validates, ignoring components");
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
            records: vec![RawEntityRecord {
                record_kind: RECORD_KIND_FULL_BASELINE,
                network_id: 3,
                baseline_id_or_ref: 1,
                new_baseline_id_or_tombstone_id: 0,
                reason: 0,
                components: vec![raw_transform_payload()],
            }],
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
            records: vec![RawEntityRecord {
                record_kind: 99, // not FullBaseline/Delta/Despawn
                network_id: 1,
                baseline_id_or_ref: 0,
                new_baseline_id_or_tombstone_id: 0,
                reason: 0,
                components: Vec::new(),
            }],
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
            records: vec![RawEntityRecord {
                record_kind: RECORD_KIND_FULL_BASELINE,
                network_id: 1,
                baseline_id_or_ref: 0,
                new_baseline_id_or_tombstone_id: 0,
                reason: 0,
                components: vec![RawComponentPayload {
                    component_kind: 1234, // not Transform/PlayerMovementState
                    transform: Some(sample_transform()),
                    player_movement: None,
                }],
            }],
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
    fn mismatched_payload_slot_rejects() {
        // Kind says PlayerMovementState, but only the Transform slot is filled.
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
                RawEntityRecord {
                    record_kind: RECORD_KIND_FULL_BASELINE,
                    network_id: 1,
                    baseline_id_or_ref: 0,
                    new_baseline_id_or_tombstone_id: 0,
                    reason: 0,
                    components: vec![raw_transform_payload()],
                },
                RawEntityRecord {
                    record_kind: 77,
                    network_id: 2,
                    baseline_id_or_ref: 0,
                    new_baseline_id_or_tombstone_id: 0,
                    reason: 0,
                    components: Vec::new(),
                },
            ],
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
                components: vec![ComponentPayload::Transform(sample_transform())],
            },
            EntityRecord::Delta {
                network_id: 1,
                baseline_ref: 2,
                new_baseline_id: 3,
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
            // that raw record must reproduce the typed variant unchanged.
            let raw = match &typed {
                EntityRecord::FullBaseline {
                    network_id,
                    baseline_id,
                    components,
                } => RawEntityRecord {
                    record_kind: RECORD_KIND_FULL_BASELINE,
                    network_id: *network_id,
                    baseline_id_or_ref: *baseline_id,
                    new_baseline_id_or_tombstone_id: 0,
                    reason: 0,
                    components: components.iter().map(raw_from_typed).collect(),
                },
                EntityRecord::Delta {
                    network_id,
                    baseline_ref,
                    new_baseline_id,
                    components,
                } => RawEntityRecord {
                    record_kind: RECORD_KIND_DELTA,
                    network_id: *network_id,
                    baseline_id_or_ref: *baseline_ref,
                    new_baseline_id_or_tombstone_id: *new_baseline_id,
                    reason: 0,
                    components: components.iter().map(raw_from_typed).collect(),
                },
                EntityRecord::Despawn {
                    network_id,
                    tombstone_id,
                    reason,
                } => RawEntityRecord {
                    record_kind: RECORD_KIND_DESPAWN,
                    network_id: *network_id,
                    baseline_id_or_ref: 0,
                    new_baseline_id_or_tombstone_id: *tombstone_id,
                    reason: *reason,
                    components: Vec::new(),
                },
            };
            // Round-trip the raw record through bitcode before validating, so the
            // pinned `record_kind` survives the wire too.
            let bytes = encode(&raw);
            let decoded: RawEntityRecord = decode(&bytes).expect("raw record decodes");
            assert_eq!(decoded.validate(), Ok(typed));
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
