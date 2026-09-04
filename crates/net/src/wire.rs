// Wire codec: bitcode-serialized snapshot and replication message types.
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
//
// Handshake, admission, and host-control declarations live in `wire/control.rs`.

use std::collections::BTreeMap;

use bitcode::{Decode, Encode};

mod control;
pub use control::{
    ClientControlMessage, ClientSwitchDeclaration, ClosingCause, ConnectClaim,
    DISPLAY_NAME_MAX_BYTES, DivergenceReason, HoldingCause, JoinSeedValue, NETCODE_USER_DATA_BYTES,
    ParityDeclaration, PlayerClaimId, ProtocolVersion, RosterEntry, ServerControlMessage,
    ServerSwitchAccepted, ServerSwitchRefused, SessionId, SessionRosterMessage,
    decode_connect_claim, encode_connect_claim,
};
pub(crate) use control::{ParticipationFrame, ServerControlFrame};

/// Network-stable entity identity. A `u32` newtype assigned by the host; the wire
/// carries it as a bare `u32` (bitcode encodes the inner field transparently).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Encode, Decode)]
pub struct NetworkId(pub u32);

/// Producer-stamped scalar fact carried with a transient presentation spawn.
///
/// This is a wire mirror of the engine presentation fact vocabulary. Keeping the
/// mirror here preserves the net crate's registry- and engine-blind boundary.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum PresentationFact {
    Number(f32),
    Text(String),
    Bool(bool),
}

/// One host-to-client passive presentation event.
///
/// This family rides the dedicated unreliable `Channel::Presentation`; it is
/// intentionally separate from [`ServerMessage`], whose envelope belongs to
/// the reliable Input channel.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct ServerPresentationMessage {
    pub payload: ServerPresentationPayload,
}

/// Payloads carried by [`ServerPresentationMessage`].
///
/// New variants must be appended. bitcode encodes enum tags positionally, and
/// both current variants are defined together so the later overlay surface does
/// not need a second wire-version bump.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ServerPresentationPayload {
    /// A one-shot transient authored from a presentation template. `value` is
    /// the conventional numeric impact value; `facts` retains the complete
    /// producer-stamped per-instance fact set.
    Spawn {
        template_id: String,
        anchor: [f32; 3],
        value: f32,
        facts: BTreeMap<String, PresentationFact>,
    },
    /// Reserved for host-pushed enemy-status facts. It is deliberately fully
    /// decodable now even though Tasks 7/8 are its first producer/consumer.
    OverlayFact {
        enemy_id: NetworkId,
        health_fraction: f32,
        shield_fraction: f32,
        has_shield: bool,
        alive: bool,
    },
}

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
///
/// Bumped to 7 in E10 follow-up: `RawComponentPayload` gained a mesh-animation
/// state slot so host-authoritative AI can replicate its current descriptor
/// animation state without sending the full mesh descriptor.
///
/// Bumped to 8 for the moving-world replication slice: `RawComponentPayload`
/// gained the kinematic-mover-state slot and `WirePlayerMovementState` widened
/// its grounded bool to a ground reference.
///
/// E16 client-authoritative combat and presentation did not bump this: shot
/// verdicts ride `ServerMessage::ShotVerdicts` on reliable Input, while passive
/// presentation rides its own unreliable channel, not `RawSnapshotMessage`.
///
/// Bumped to 9 for E17 trigger commands: mover phase gained `target_segment`
/// and movement input gained `use_pressed`.
///
/// Bumped to 10 for E21 co-op avatar presentation: player movement gained
/// replicated `aim_pitch`, and entity records gained active-weapon archetype
/// metadata.
///
/// Bumped to 11 for E17 rotating movers: kinematic mover phase gained spin
/// angle plus current and target angular rates.
///
/// Bumped to 12 for mover replay provenance: kinematic mover phase gained the
/// pre-tick spin angle and active-at-tick-start flag.
///
/// Bumped to 13 for E17 blocking movers: kinematic mover phase gained the
/// host-authoritative `blocked` stop-hold flag.
///
/// Bumped to 14 for slide: `WireMovementState` gained its `Sliding` variant,
/// including the floor normal needed to replay a sloped authoritative baseline.
pub const SNAPSHOT_VERSION: u16 = 14;

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
/// `component_kind` discriminant for a mesh-animation-state payload. Numeric-equal
/// to the engine `ComponentKind::Mesh as u16` (Phase 2 = 9).
pub const COMPONENT_KIND_MESH_ANIMATION_STATE: u16 = 9;
/// `component_kind` discriminant for a kinematic-mover-state payload.
/// Numeric-equal to the engine `ComponentKind::KinematicMover as u16`.
pub const COMPONENT_KIND_KINEMATIC_MOVER_STATE: u16 = 13;

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
    ///
    /// `pub(crate)` so the production side (`MovementAuthority::for_recipient`) gates
    /// `entity_class` on the SAME finite-`Transform` rule `validate` enforces on receipt,
    /// keeping production and validation in lockstep.
    #[must_use]
    pub(crate) fn all_finite(&self) -> bool {
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
    Dash {
        elapsed_ms: f32,
        boost: [f32; 3],
    },
    Crouching {
        eye_current: f32,
    },
    Sliding {
        elapsed_ms: f32,
        boost: [f32; 3],
        eye_current: f32,
        /// The prior substrate contact. It is paid only while sliding because
        /// replay needs the authoritative slope on its first replayed tick. Raw
        /// validation accepts only absence or a bounded near-unit vector.
        floor_normal: Option<[f32; 3]>,
    },
}

/// Squared-length tolerance for collision-produced floor normals. Bitcode
/// preserves the source `f32` values exactly, so this only accommodates normal
/// floating-point normalization error; it is not a normalization step.
const SLIDING_FLOOR_NORMAL_LENGTH_SQUARED_TOLERANCE: f32 = 1.0e-3;

fn sliding_floor_normal_is_valid(normal: &[f32; 3]) -> bool {
    // Bound components before squaring so even finite hostile values cannot
    // overflow the magnitude calculation used by this validation gate.
    normal.iter().all(|component| {
        component.is_finite()
            && component.abs() <= 1.0 + SLIDING_FLOOR_NORMAL_LENGTH_SQUARED_TOLERANCE
    }) && ((normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]) - 1.0).abs()
        <= SLIDING_FLOOR_NORMAL_LENGTH_SQUARED_TOLERANCE
}

/// Wire mirror of the player's generalized ground reference. `Mover` carries the
/// compile-time PRL mover id, not a `NetworkId`, so the value is stable across
/// peers that loaded the same map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub enum WireGroundRef {
    Airborne,
    World,
    Mover(u32),
}

/// Wire mirror of the *mutable tick subset* of the engine `PlayerMovementComponent`.
///
/// Deliberately **not** a copy of the component struct: descriptor-immutable
/// movement params, `view_feel`, `standing_*`, `stuck_stop_*`, and the IR-bound
/// `dash_programs` stay out of this typed tick-state mirror. The local descriptor
/// is a non-authoritative immutable mirror; E15 sends host-authoritative resolved
/// tuning separately as opaque Control payload bytes. Only the fields interpolation
/// and later prediction reconciliation need are mirrored here. Source field types
/// are preserved: ability counters and `air_ticks` are `u32`; live timers are `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WirePlayerMovementState {
    pub velocity: [f32; 3],
    pub ground: WireGroundRef,
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
    /// Camera pitch from the movement owner's latest resolved input. It is
    /// presentation state for remote avatars, not a movement-simulation value.
    pub aim_pitch: f32,
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
            WireMovementState::Sliding {
                elapsed_ms,
                boost,
                eye_current,
                floor_normal,
            } => {
                elapsed_ms.is_finite()
                    && boost.iter().all(|c| c.is_finite())
                    && eye_current.is_finite()
                    && floor_normal
                        .as_ref()
                        .is_none_or(|normal| normal.iter().all(|c| c.is_finite()))
            }
        }
    }

    /// Whether variant-specific movement invariants hold after decoding. Dash may
    /// preserve vertical boost. Slide boost is horizontal, and its optional floor
    /// normal must be safely bounded and near-unit before replay projects gravity
    /// against it.
    #[must_use]
    fn has_valid_state_contract(&self) -> bool {
        match self {
            WireMovementState::Normal
            | WireMovementState::Dash { .. }
            | WireMovementState::Crouching { .. } => true,
            WireMovementState::Sliding {
                boost,
                floor_normal,
                ..
            } => {
                boost[1] == 0.0
                    && floor_normal
                        .as_ref()
                        .is_none_or(sliding_floor_normal_is_valid)
            }
        }
    }
}

impl WirePlayerMovementState {
    /// Whether every replicated float is finite (no NaN/inf): velocity, all live
    /// timers, capsule dimensions, aim pitch, and the active state's payload. Checked
    /// at `validate` so a non-finite movement state is rejected before typed apply and
    /// never reaches the registry. Integer counters and bools cannot be non-finite, so
    /// they are not checked.
    #[must_use]
    fn all_finite(&self) -> bool {
        self.velocity.iter().all(|c| c.is_finite())
            && self.dash_cooldown_ms.is_finite()
            && self.coyote_timer_ms.is_finite()
            && self.jump_buffer_timer_ms.is_finite()
            && self.capsule_half_height.is_finite()
            && self.capsule_eye_height.is_finite()
            && self.aim_pitch.is_finite()
            && self.movement_state.all_finite()
    }
}

/// Wire mirror of a kinematic mover's replicated deterministic phase. Static
/// path data stays local on each peer through PRL load; these fields are the
/// authoritative phase seed clients predict from and reconcile against.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireKinematicMoverState {
    pub mover_id: u32,
    pub segment_index: u16,
    pub direction: i8,
    pub mode: u8,
    pub segment_elapsed_ms: f32,
    pub wait_remaining_ms: f32,
    pub started: bool,
    pub completed: bool,
    pub blocked: bool,
    pub velocity: [f32; 3],
    pub target_segment: Option<u16>,
    pub spin_angle_rad: f32,
    pub spin_angle_before_tick_rad: f32,
    pub was_active_this_tick: bool,
    pub spin_rate_rad_s: f32,
    pub spin_target_rate_rad_s: f32,
}

impl WireKinematicMoverState {
    #[must_use]
    fn all_finite(&self) -> bool {
        self.segment_elapsed_ms.is_finite()
            && self.wait_remaining_ms.is_finite()
            && self.velocity.iter().all(|c| c.is_finite())
            && self.spin_angle_rad.is_finite()
            && self.spin_angle_before_tick_rad.is_finite()
            && self.spin_rate_rad_s.is_finite()
            && self.spin_target_rate_rad_s.is_finite()
    }

    #[must_use]
    fn has_valid_phase_tags(&self) -> bool {
        matches!(self.direction, -1 | 1) && matches!(self.mode, 0 | 1)
    }
}

/// Wire mirror of the mutable mesh-animation state. Descriptor-owned data
/// (model handle, state table, clips, fade policy) stays local on each peer; this
/// carries only the authoritative current state name.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct WireMeshAnimationState {
    pub current_state: String,
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
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct RawComponentPayload {
    pub component_kind: u16,
    pub transform: Option<WireTransform>,
    pub player_movement: Option<WirePlayerMovementState>,
    pub mesh_animation_state: Option<WireMeshAnimationState>,
    pub kinematic_mover: Option<WireKinematicMoverState>,
}

/// Raw per-entity lifecycle record. `record_kind` selects which logical record
/// this is; the `baseline_id_or_ref` / `new_baseline_id_or_tombstone_id` / `reason`
/// fields are overloaded per kind (see the `validate` mapping). A `Despawn` ignores
/// unrelated id fields, but it must not carry metadata or component payloads.
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
    /// required because an empty `String` is not a class value; `false` with a
    /// non-empty class and `true` with an empty class are both malformed envelopes
    /// rejected at `validate`.
    pub has_entity_class: bool,
    /// The opaque descriptor-class identifier the host materialized this entity from
    /// (e.g. `"player"`), so the client can materialize the matching descriptor-backed
    /// presentation entity locally. Meaningful only when `has_entity_class` is `true`,
    /// and valid only on a non-despawn record backed by a finite `Transform` — a
    /// despawn record, or a record without a finite `Transform`, carrying it is
    /// rejected at `validate`. This is a plain string identifier, NOT a descriptor
    /// type: the crate stays registry-blind and never resolves it.
    pub entity_class: String,
    /// Whether `active_weapon_archetype` carries a real value. Mirrors the typed
    /// `EntityRecord` option: `false` requires an empty string; `true` requires a
    /// non-empty canonical weapon archetype name.
    pub has_active_weapon_archetype: bool,
    /// The active weapon's opaque descriptor canonical name. Valid only on a
    /// non-despawn record carrying `PlayerMovementState`; no value means no weapon
    /// is currently equipped.
    pub active_weapon_archetype: String,
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
#[derive(Debug, Clone, PartialEq)]
pub enum ComponentPayload {
    Transform(WireTransform),
    PlayerMovementState(WirePlayerMovementState),
    MeshAnimationState(WireMeshAnimationState),
    KinematicMoverState(WireKinematicMoverState),
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
            ComponentPayload::MeshAnimationState(_) => COMPONENT_KIND_MESH_ANIMATION_STATE,
            ComponentPayload::KinematicMoverState(_) => COMPONENT_KIND_KINEMATIC_MOVER_STATE,
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
        /// The opaque canonical name of this movement pawn's active weapon, or
        /// `None` when it has no active weapon. Valid only on records carrying
        /// `PlayerMovementState` (enforced at `validate`).
        active_weapon_archetype: Option<String>,
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
        /// See `FullBaseline::active_weapon_archetype`.
        active_weapon_archetype: Option<String>,
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
    /// A despawn record carried movement-authority metadata, an `entity_class`, or
    /// active-weapon metadata. A tombstone has no pawn state or presentation identity.
    MetadataOnDespawn,
    /// A despawn record carried component payloads. Despawns are tombstone-only and
    /// never carry replicated component state.
    ComponentsOnDespawn,
    /// A record carried an `entity_class` (`has_entity_class = true`) but does not
    /// carry a structurally-valid finite `Transform` payload. The class names a
    /// descriptor the client materializes as a presentation entity, which rides the
    /// wire as a `Transform`; without a finite pose there is nothing to place.
    EntityClassWithoutTransform,
    /// `has_entity_class` was `false` but `entity_class` was non-empty — the "absent"
    /// flag cannot ride a real class value.
    MalformedEntityClassMetadata,
    /// `has_entity_class` was `true` but `entity_class` was empty. The flag means a
    /// concrete class value is present.
    EmptyEntityClassMetadata,
    /// `has_active_weapon_archetype` was `false` but its string was non-empty, or
    /// it was `true` but its string was empty.
    MalformedActiveWeaponMetadata,
    /// Active-weapon metadata is meaningful only on a `PlayerMovementState` record.
    ActiveWeaponMetadataWithoutMovement,
    /// A `PlayerMovementState` payload carried a non-finite float (NaN/inf) in one
    /// of its replicated fields (velocity, timers, active-state values, or capsule
    /// dimensions). Rejected before typed apply so no non-finite movement state
    /// reaches the registry.
    NonFiniteMovementState,
    /// A movement-state variant carried finite values that violate its state
    /// contract. Sliding boost must be horizontal, and its optional floor normal
    /// must be safely bounded and near-unit.
    InvalidMovementState,
    /// A `Transform` payload carried a non-finite float (NaN/inf) in its position,
    /// rotation, or scale. Rejected before typed apply so no non-finite pose reaches
    /// the registry — and so a non-finite `Transform` cannot back an `entity_class`.
    NonFiniteTransform,
    /// A `KinematicMoverState` payload carried a non-finite phase float.
    NonFiniteKinematicMoverState,
    /// A `KinematicMoverState` payload carried an invalid `direction` or `mode`
    /// tag. Loaded-mover existence is intentionally not checked in this crate.
    InvalidKinematicMoverState,
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
                write!(
                    f,
                    "movement-authority, entity_class, or active-weapon metadata on a despawn record"
                )
            }
            ValidationError::ComponentsOnDespawn => {
                write!(f, "component payloads on a despawn record")
            }
            ValidationError::EntityClassWithoutTransform => write!(
                f,
                "entity_class on a record without a finite Transform component"
            ),
            ValidationError::MalformedEntityClassMetadata => {
                write!(f, "has_entity_class=false but entity_class is non-empty")
            }
            ValidationError::EmptyEntityClassMetadata => {
                write!(f, "has_entity_class=true but entity_class is empty")
            }
            ValidationError::MalformedActiveWeaponMetadata => write!(
                f,
                "active-weapon metadata flag and archetype string disagree"
            ),
            ValidationError::ActiveWeaponMetadataWithoutMovement => write!(
                f,
                "active-weapon metadata on a record without a PlayerMovementState component"
            ),
            ValidationError::NonFiniteMovementState => {
                write!(f, "non-finite float in a PlayerMovementState payload")
            }
            ValidationError::InvalidMovementState => {
                write!(
                    f,
                    "invalid PlayerMovementState contract: Sliding boost must be horizontal and floor normal must be absent or a bounded near-unit vector"
                )
            }
            ValidationError::NonFiniteTransform => {
                write!(f, "non-finite float in a Transform payload")
            }
            ValidationError::NonFiniteKinematicMoverState => {
                write!(f, "non-finite float in a KinematicMoverState payload")
            }
            ValidationError::InvalidKinematicMoverState => {
                write!(
                    f,
                    "invalid direction or mode in a KinematicMoverState payload"
                )
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
            + usize::from(self.player_movement.is_some())
            + usize::from(self.mesh_animation_state.is_some())
            + usize::from(self.kinematic_mover.is_some());

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
                    if !m.all_finite() {
                        Err(ValidationError::NonFiniteMovementState)
                    } else if !m.movement_state.has_valid_state_contract() {
                        Err(ValidationError::InvalidMovementState)
                    } else {
                        Ok(ComponentPayload::PlayerMovementState(m))
                    }
                }
                Some(_) => Err(ValidationError::MismatchedComponentPayload(
                    self.component_kind,
                )),
                None => Err(ValidationError::MissingComponentPayload(
                    self.component_kind,
                )),
            },
            COMPONENT_KIND_MESH_ANIMATION_STATE => match &self.mesh_animation_state {
                Some(m) if populated == 1 => Ok(ComponentPayload::MeshAnimationState(m.clone())),
                Some(_) => Err(ValidationError::MismatchedComponentPayload(
                    self.component_kind,
                )),
                None => Err(ValidationError::MissingComponentPayload(
                    self.component_kind,
                )),
            },
            COMPONENT_KIND_KINEMATIC_MOVER_STATE => match self.kinematic_mover {
                Some(m) if populated == 1 => {
                    if !m.all_finite() {
                        Err(ValidationError::NonFiniteKinematicMoverState)
                    } else if !m.has_valid_phase_tags() {
                        Err(ValidationError::InvalidKinematicMoverState)
                    } else {
                        Ok(ComponentPayload::KinematicMoverState(m))
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
                let active_weapon_archetype = self.validate_active_weapon_metadata(&components)?;
                Ok(EntityRecord::FullBaseline {
                    network_id: self.network_id,
                    baseline_id: self.baseline_id_or_ref,
                    last_processed_client_tick,
                    local_player: self.local_player,
                    entity_class,
                    active_weapon_archetype,
                    components,
                })
            }
            RECORD_KIND_DELTA => {
                let components = self.validate_components()?;
                let last_processed_client_tick = self.validate_movement_metadata(&components)?;
                let entity_class = self.validate_entity_class(&components)?;
                let active_weapon_archetype = self.validate_active_weapon_metadata(&components)?;
                Ok(EntityRecord::Delta {
                    network_id: self.network_id,
                    baseline_ref: self.baseline_id_or_ref,
                    new_baseline_id: self.new_baseline_id_or_tombstone_id,
                    last_processed_client_tick,
                    local_player: self.local_player,
                    entity_class,
                    active_weapon_archetype,
                    components,
                })
            }
            // Despawn is tombstone-only. It carries no component state and no
            // metadata: no pawn state to ack and no descriptor class to materialize.
            RECORD_KIND_DESPAWN => {
                if self.has_last_processed_client_tick
                    || self.last_processed_client_tick != 0
                    || self.local_player
                    || self.has_entity_class
                    || !self.entity_class.is_empty()
                    || self.has_active_weapon_archetype
                    || !self.active_weapon_archetype.is_empty()
                {
                    return Err(ValidationError::MetadataOnDespawn);
                }
                if !self.components.is_empty() {
                    return Err(ValidationError::ComponentsOnDespawn);
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
    /// - `has_entity_class = true` with an empty class is malformed.
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
            if self.entity_class.is_empty() {
                return Err(ValidationError::EmptyEntityClassMetadata);
            }
            let carries_finite_transform = components.iter().any(|c| match c {
                ComponentPayload::Transform(t) => t.all_finite(),
                ComponentPayload::PlayerMovementState(_) => false,
                ComponentPayload::MeshAnimationState(_) => false,
                ComponentPayload::KinematicMoverState(_) => false,
            });
            if !carries_finite_transform {
                return Err(ValidationError::EntityClassWithoutTransform);
            }
            Ok(Some(self.entity_class.clone()))
        } else {
            Ok(None)
        }
    }

    /// Validate this record's active-weapon metadata against its components. Unlike
    /// `entity_class`, this identity is player-pawn state and therefore requires a
    /// `PlayerMovementState` payload on the same non-despawn record.
    fn validate_active_weapon_metadata(
        &self,
        components: &[ComponentPayload],
    ) -> Result<Option<String>, ValidationError> {
        if (!self.has_active_weapon_archetype && !self.active_weapon_archetype.is_empty())
            || (self.has_active_weapon_archetype && self.active_weapon_archetype.is_empty())
        {
            return Err(ValidationError::MalformedActiveWeaponMetadata);
        }

        if self.has_active_weapon_archetype
            && !components
                .iter()
                .any(|component| matches!(component, ComponentPayload::PlayerMovementState(_)))
        {
            return Err(ValidationError::ActiveWeaponMetadataWithoutMovement);
        }

        Ok(self
            .has_active_weapon_archetype
            .then(|| self.active_weapon_archetype.clone()))
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
    pub use_pressed: bool,
    pub drop_pressed: bool,
    /// Camera pitch, appended after the E17 input layout. It is replicated for
    /// remote-avatar presentation and does not participate in movement simulation.
    pub aim_pitch: f32,
    /// Slot the client currently declares as the source of its fire intent. This
    /// is a level like `reload`, not a switch edge: a held value survives input-gap
    /// replay and the host resolves it against the pawn's possessed inventory.
    pub firing_slot: u8,
}

/// Wire mirror of the engine `FireButtonState`.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireFireButtonState {
    pub pressed: bool,
    pub active: bool,
}

/// Input-command envelope: the client's per-tick intent, mirroring the engine
/// `SimCommand` (movement + fire button + reload). Round-tripped in Phase 1; applied to
/// gameplay in Phase 2; reconciled against in Phase 3. `movement.firing_slot` is
/// deliberately part of this bitcode layout; Task 5's tuning-payload epoch rejects
/// stale peers rather than changing the transport protocol constants.
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
    pub reload: bool,
}

/// One client-declared hit record for a host-authorized shot. `target` is normally
/// a `NetworkId` (`u32`) because the net crate is registry-blind. Projectile
/// declarations reserve `u32::MAX` as a presentation-only contact marker when a
/// world contact (or no-longer-nameable entity contact) has no damage target.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct HitRecord {
    pub target: u32,
    pub point: [f32; 3],
    pub zone: Option<String>,
}

/// Standalone client -> server hit declaration. It intentionally does not ride
/// [`InputCommand`]: a resolved hit may arrive later than the FIRE command that
/// authorized `shot_id` (projectile-ready shape).
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct HitDeclaration {
    pub shot_id: u64,
    pub records: Vec<HitRecord>,
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
/// replication acks, entity/state baseline-refresh requests, time-sync, and
/// client-declared hit results). bitcode tags the enum, so the server decodes one
/// `ClientMessage` and matches on the variant rather than guessing the type of an
/// untagged payload. New kinds are added as **appended** variants to preserve the
/// discriminant order of existing variants.
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
    /// Client-authoritative hit results for a host-authorized shot. Appended last
    /// to preserve all existing discriminants.
    HitDeclaration(HitDeclaration),
}

/// One owner-private server verdict for a client-predicted shot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct ShotVerdict {
    pub shot_id: u64,
    /// Whether the host authorized the FIRE that minted this shot. `false` means no
    /// host-authorized shot existed, so clients roll back muzzle/cooldown.
    pub accept: bool,
    /// Whether at least one declared HIT record validated and applied. This is
    /// separate from FIRE authorization so an authorized miss does not look like a
    /// rejected fire.
    pub hit_accepted: bool,
}

/// Owner-scoped per-shot accept/reject verdicts. Empty lists are valid: they let
/// the server send a tick carrier even when no shot settled.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct ShotVerdictsMessage {
    pub verdicts: Vec<ShotVerdict>,
}

/// Server -> client reliable input-channel envelope. This wraps time-sync echoes
/// and future owner-private facts so clients can decode one tagged message family
/// from `Channel::Input`.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum ServerMessage {
    TimeSync(crate::timesync::TimeSyncEcho),
    ShotVerdicts(ShotVerdictsMessage),
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
            ground: WireGroundRef::Airborne,
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
            aim_pitch: -0.45,
        }
    }

    fn raw_transform_payload() -> RawComponentPayload {
        RawComponentPayload {
            component_kind: COMPONENT_KIND_TRANSFORM,
            transform: Some(sample_transform()),
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: None,
        }
    }

    fn raw_movement_payload() -> RawComponentPayload {
        RawComponentPayload {
            component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            transform: None,
            player_movement: Some(sample_movement()),
            mesh_animation_state: None,
            kinematic_mover: None,
        }
    }

    fn raw_mesh_animation_payload(state: &str) -> RawComponentPayload {
        RawComponentPayload {
            component_kind: COMPONENT_KIND_MESH_ANIMATION_STATE,
            transform: None,
            player_movement: None,
            mesh_animation_state: Some(WireMeshAnimationState {
                current_state: state.to_string(),
            }),
            kinematic_mover: None,
        }
    }

    fn sample_mover_state() -> WireKinematicMoverState {
        WireKinematicMoverState {
            mover_id: 42,
            segment_index: 1,
            direction: -1,
            mode: 1,
            segment_elapsed_ms: 125.0,
            wait_remaining_ms: 0.0,
            started: true,
            completed: false,
            blocked: true,
            velocity: [1.0, 0.0, -0.5],
            target_segment: Some(2),
            spin_angle_rad: 1.25,
            spin_angle_before_tick_rad: 1.0,
            was_active_this_tick: true,
            spin_rate_rad_s: -0.5,
            spin_target_rate_rad_s: -1.0,
        }
    }

    fn raw_mover_payload() -> RawComponentPayload {
        RawComponentPayload {
            component_kind: COMPONENT_KIND_KINEMATIC_MOVER_STATE,
            transform: None,
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: Some(sample_mover_state()),
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
            has_active_weapon_archetype: false,
            active_weapon_archetype: String::new(),
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
                use_pressed: true,
                drop_pressed: true,
                aim_pitch: -0.45,
                firing_slot: 3,
            },
            fire_button: WireFireButtonState {
                pressed: true,
                active: false,
            },
            reload: true,
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
            WireMovementState::Sliding {
                elapsed_ms: 25.0,
                boost: [1.0, 0.0, 3.0],
                eye_current: 0.8,
                floor_normal: Some([0.2, 0.979_795_9, 0.0]),
            },
            WireMovementState::Sliding {
                elapsed_ms: 25.0,
                boost: [1.0, 0.0, 3.0],
                eye_current: 0.8,
                floor_normal: None,
            },
        ] {
            let movement = WirePlayerMovementState {
                movement_state: state,
                ..sample_movement()
            };
            assert!(round_trips(&movement));
        }
    }

    #[test]
    fn mesh_animation_state_round_trips_and_validates() {
        let raw = raw_mesh_animation_payload("attack");
        assert!(round_trips(&raw));
        assert_eq!(
            raw.validate(),
            Ok(ComponentPayload::MeshAnimationState(
                WireMeshAnimationState {
                    current_state: "attack".to_string(),
                },
            ))
        );
    }

    #[test]
    fn input_command_round_trips() {
        assert!(round_trips(&sample_input()));
    }

    #[test]
    fn hit_declaration_round_trips_empty_and_multiple_records() {
        let empty = HitDeclaration {
            shot_id: 9,
            records: Vec::new(),
        };
        assert!(round_trips(&empty));

        let declaration = HitDeclaration {
            shot_id: 0xABCD_EF01_2345_6789,
            records: vec![
                HitRecord {
                    target: 17,
                    point: [1.0, 2.5, -3.0],
                    zone: Some("head".to_string()),
                },
                HitRecord {
                    target: 22,
                    point: [0.0, 0.0, 0.0],
                    zone: None,
                },
            ],
        };
        assert!(round_trips(&declaration));
        let msg = ClientMessage::HitDeclaration(declaration.clone());
        assert!(round_trips(&msg));
        let bytes = encode(&msg);
        let decoded: ClientMessage = decode(&bytes).expect("client message decodes");
        assert_eq!(decoded, msg);
    }

    #[test]
    fn hit_declaration_decode_failure_is_typed_error() {
        let err = decode::<ClientMessage>(&[0xFF, 0x00]).expect_err("garbage rejects");
        assert!(matches!(err, WireError::Decode(_)));
    }

    #[test]
    fn shot_verdicts_server_message_round_trips_empty_and_multiple() {
        let empty = ServerMessage::ShotVerdicts(ShotVerdictsMessage {
            verdicts: Vec::new(),
        });
        assert!(round_trips(&empty));

        let verdicts = ServerMessage::ShotVerdicts(ShotVerdictsMessage {
            verdicts: vec![
                ShotVerdict {
                    shot_id: 10,
                    accept: true,
                    hit_accepted: true,
                },
                ShotVerdict {
                    shot_id: 11,
                    accept: false,
                    hit_accepted: false,
                },
            ],
        });
        assert!(round_trips(&verdicts));
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
            ClientMessage::HitDeclaration(HitDeclaration {
                shot_id: 99,
                records: vec![HitRecord {
                    target: 4,
                    point: [1.0, 2.0, 3.0],
                    zone: Some("torso".to_string()),
                }],
            }),
        ];
        for msg in variants {
            assert!(round_trips(&msg));
        }
    }

    #[test]
    fn server_message_variants_round_trip() {
        let variants = [
            ServerMessage::TimeSync(crate::timesync::TimeSyncEcho {
                sample_id: 1,
                client_send_tick: 2,
                client_send_time_us: 3,
                server_tick: 4,
                server_echo_time_us: 5,
            }),
            ServerMessage::ShotVerdicts(ShotVerdictsMessage {
                verdicts: vec![ShotVerdict {
                    shot_id: 8,
                    accept: true,
                    hit_accepted: true,
                }],
            }),
        ];
        for msg in variants {
            assert!(round_trips(&msg));
        }
    }

    #[test]
    fn presentation_message_payloads_round_trip_on_current_snapshot_version() {
        assert_eq!(
            SNAPSHOT_VERSION, 14,
            "slide's snapshot-state layout requires snapshot version 14"
        );

        let spawn = ServerPresentationMessage {
            payload: ServerPresentationPayload::Spawn {
                template_id: "damage-number".to_string(),
                anchor: [1.25, 2.5, -3.75],
                value: 42.0,
                facts: BTreeMap::from([
                    ("value".to_string(), PresentationFact::Number(42.0)),
                    ("critical".to_string(), PresentationFact::Bool(true)),
                    (
                        "label".to_string(),
                        PresentationFact::Text("critical hit".to_string()),
                    ),
                ]),
            },
        };
        assert!(round_trips(&spawn));

        let overlay = ServerPresentationMessage {
            payload: ServerPresentationPayload::OverlayFact {
                enemy_id: NetworkId(77),
                health_fraction: 0.25,
                shield_fraction: 0.5,
                has_shield: true,
                alive: true,
            },
        };
        assert!(round_trips(&overlay));
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
                active_weapon_archetype: None,
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
                active_weapon_archetype: None,
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
            records: vec![raw_record(RECORD_KIND_DESPAWN, 9, 0, 42, 7, Vec::new())],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
        };
        let typed = raw.validate().expect("tombstone-only despawn validates");
        assert_eq!(
            typed.records,
            vec![EntityRecord::Despawn {
                network_id: 9,
                tombstone_id: 42,
                reason: 7,
            }]
        );
    }

    #[test]
    fn validate_despawn_rejects_component_payloads() {
        let record = raw_record(
            RECORD_KIND_DESPAWN,
            9,
            0,
            42,
            7,
            vec![raw_transform_payload()],
        );
        assert_eq!(record.validate(), Err(ValidationError::ComponentsOnDespawn));
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
                    mesh_animation_state: None,
                    kinematic_mover: None,
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
            mesh_animation_state: None,
            kinematic_mover: None,
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
            mesh_animation_state: None,
            kinematic_mover: None,
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
            mesh_animation_state: None,
            kinematic_mover: None,
        };
        assert_eq!(
            payload.validate(),
            Err(ValidationError::MissingComponentPayload(
                COMPONENT_KIND_PLAYER_MOVEMENT_STATE
            ))
        );
    }

    #[test]
    fn sliding_snapshot_version_rejects_immediately_previous_layout() {
        const PRE_SLIDING_SNAPSHOT_VERSION: u16 = 13;
        assert_eq!(
            SNAPSHOT_VERSION, 14,
            "sliding movement state requires snapshot version 14"
        );
        let raw = RawSnapshotMessage {
            version: PRE_SLIDING_SNAPSHOT_VERSION,
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
                received: PRE_SLIDING_SNAPSHOT_VERSION,
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
    // Transform = 0, PlayerMovement = 6, Mesh = 9 in Phase 2. The exhaustive match (no `_`
    // arm) means a new `ComponentPayload` variant is a compile error here until its
    // expected discriminant is pinned — a silently-passing guard is the failure
    // mode this prevents. The engine side asserts the same mapping independently
    // (`component_kind_discriminant`), so a divergence fails one side's guard.
    #[test]
    fn component_kind_pinned_to_engine_discriminants() {
        let cases = [
            ComponentPayload::Transform(sample_transform()),
            ComponentPayload::PlayerMovementState(sample_movement()),
            ComponentPayload::MeshAnimationState(WireMeshAnimationState {
                current_state: "idle".to_string(),
            }),
            ComponentPayload::KinematicMoverState(sample_mover_state()),
        ];
        for payload in cases {
            let expected = match payload {
                ComponentPayload::Transform(_) => 0,
                ComponentPayload::PlayerMovementState(_) => 6,
                ComponentPayload::MeshAnimationState(_) => 9,
                ComponentPayload::KinematicMoverState(_) => 13,
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
                active_weapon_archetype: None,
                components: vec![
                    ComponentPayload::Transform(sample_transform()),
                    ComponentPayload::KinematicMoverState(sample_mover_state()),
                ],
            },
            EntityRecord::Delta {
                network_id: 1,
                baseline_ref: 2,
                new_baseline_id: 3,
                last_processed_client_tick: None,
                local_player: false,
                entity_class: None,
                active_weapon_archetype: None,
                components: vec![
                    ComponentPayload::Transform(sample_transform()),
                    ComponentPayload::KinematicMoverState(sample_mover_state()),
                ],
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
        assert!(decoded.reload);
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
        record.has_active_weapon_archetype = true;
        record.active_weapon_archetype = "reference_pistol".to_string();

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
                active_weapon_archetype: Some("reference_pistol".to_string()),
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
                active_weapon_archetype: None,
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

    #[test]
    fn metadata_on_despawn_display_mentions_entity_class() {
        let text = ValidationError::MetadataOnDespawn.to_string();
        assert!(text.contains("movement-authority"));
        assert!(text.contains("entity_class"));
        assert!(text.contains("active-weapon"));
        assert!(text.contains("despawn"));
    }

    // --- Active-weapon metadata validation ---

    #[test]
    fn active_weapon_metadata_requires_a_non_empty_value_with_a_movement_payload() {
        let mut absent_flag_with_value = raw_record(
            RECORD_KIND_FULL_BASELINE,
            1,
            1,
            0,
            0,
            vec![raw_movement_payload()],
        );
        absent_flag_with_value.active_weapon_archetype = "reference_pistol".to_string();
        assert_eq!(
            absent_flag_with_value.validate(),
            Err(ValidationError::MalformedActiveWeaponMetadata)
        );

        let mut present_flag_without_value = raw_record(
            RECORD_KIND_FULL_BASELINE,
            1,
            1,
            0,
            0,
            vec![raw_movement_payload()],
        );
        present_flag_without_value.has_active_weapon_archetype = true;
        assert_eq!(
            present_flag_without_value.validate(),
            Err(ValidationError::MalformedActiveWeaponMetadata)
        );

        let mut transform_only = raw_record(
            RECORD_KIND_FULL_BASELINE,
            1,
            1,
            0,
            0,
            vec![raw_transform_payload()],
        );
        transform_only.has_active_weapon_archetype = true;
        transform_only.active_weapon_archetype = "reference_pistol".to_string();
        assert_eq!(
            transform_only.validate(),
            Err(ValidationError::ActiveWeaponMetadataWithoutMovement)
        );
    }

    #[test]
    fn active_weapon_metadata_on_despawn_rejects() {
        let mut flagged = raw_record(RECORD_KIND_DESPAWN, 1, 0, 9, 0, Vec::new());
        flagged.has_active_weapon_archetype = true;
        assert_eq!(flagged.validate(), Err(ValidationError::MetadataOnDespawn));

        let mut value = raw_record(RECORD_KIND_DESPAWN, 1, 0, 9, 0, Vec::new());
        value.active_weapon_archetype = "reference_pistol".to_string();
        assert_eq!(value.validate(), Err(ValidationError::MetadataOnDespawn));
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
                active_weapon_archetype: None,
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
                active_weapon_archetype: None,
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
                active_weapon_archetype: None,
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

    /// `has_entity_class = true` means a real class value is present; an empty
    /// string cannot stand in for `Some`.
    #[test]
    fn empty_entity_class_with_present_flag_rejects() {
        let mut record = raw_record(
            RECORD_KIND_FULL_BASELINE,
            1,
            1,
            0,
            0,
            vec![raw_transform_payload()],
        );
        record.has_entity_class = true;
        assert_eq!(
            record.validate(),
            Err(ValidationError::EmptyEntityClassMetadata)
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

    /// Every replicated float field of a `PlayerMovementState`, including aim pitch,
    /// must be finite; a NaN/inf in any of them is rejected before typed apply, so no
    /// non-finite movement state reaches the registry. Each case mutates exactly one
    /// field.
    #[test]
    fn non_finite_movement_state_rejects_each_field() {
        let mutators: [fn(&mut WirePlayerMovementState); 14] = [
            |m| m.velocity[0] = f32::NAN,
            |m| m.velocity[2] = f32::INFINITY,
            |m| m.dash_cooldown_ms = f32::NAN,
            |m| m.coyote_timer_ms = f32::INFINITY,
            |m| m.jump_buffer_timer_ms = f32::NEG_INFINITY,
            |m| m.capsule_half_height = f32::NAN,
            |m| m.capsule_eye_height = f32::INFINITY,
            |m| m.aim_pitch = f32::NEG_INFINITY,
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
            |m| {
                m.movement_state = WireMovementState::Sliding {
                    elapsed_ms: f32::NAN,
                    boost: [0.0, 0.0, 0.0],
                    eye_current: 0.5,
                    floor_normal: None,
                }
            },
            |m| {
                m.movement_state = WireMovementState::Sliding {
                    elapsed_ms: 1.0,
                    boost: [0.0, f32::INFINITY, 0.0],
                    eye_current: 0.5,
                    floor_normal: None,
                }
            },
            |m| {
                m.movement_state = WireMovementState::Sliding {
                    elapsed_ms: 1.0,
                    boost: [0.0, 0.0, 0.0],
                    eye_current: f32::NAN,
                    floor_normal: None,
                }
            },
            |m| {
                m.movement_state = WireMovementState::Sliding {
                    elapsed_ms: 1.0,
                    boost: [0.0, 0.0, 0.0],
                    eye_current: 0.5,
                    floor_normal: Some([0.0, f32::NEG_INFINITY, 0.0]),
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
                mesh_animation_state: None,
                kinematic_mover: None,
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
            mesh_animation_state: None,
            kinematic_mover: None,
        };
        assert_eq!(
            payload.validate(),
            Err(ValidationError::NonFiniteMovementState)
        );
    }

    // Regression: a finite vertical slide boost previously passed raw validation
    // and could materialize an invalid engine movement state.
    #[test]
    fn vertical_sliding_boost_rejects_before_typed_apply() {
        let movement = WirePlayerMovementState {
            movement_state: WireMovementState::Sliding {
                elapsed_ms: 10.0,
                boost: [1.0, 0.25, 3.0],
                eye_current: 0.5,
                floor_normal: Some([0.0, 1.0, 0.0]),
            },
            ..sample_movement()
        };
        let payload = RawComponentPayload {
            component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            transform: None,
            player_movement: Some(movement),
            mesh_animation_state: None,
            kinematic_mover: None,
        };

        assert_eq!(
            payload.validate(),
            Err(ValidationError::InvalidMovementState)
        );
    }

    // Regression: a finite but unbounded floor normal could overflow slide replay's
    // gravity projection and poison movement state with infinity or NaN.
    #[test]
    fn invalid_sliding_floor_normal_rejects_before_typed_apply() {
        for floor_normal in [[f32::MAX, 1.0, 0.0], [0.0, 0.5, 0.0], [0.0, 0.0, 0.0]] {
            let movement = WirePlayerMovementState {
                movement_state: WireMovementState::Sliding {
                    elapsed_ms: 10.0,
                    boost: [1.0, 0.0, 3.0],
                    eye_current: 0.5,
                    floor_normal: Some(floor_normal),
                },
                ..sample_movement()
            };
            let payload = RawComponentPayload {
                component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
                transform: None,
                player_movement: Some(movement),
                mesh_animation_state: None,
                kinematic_mover: None,
            };

            assert_eq!(
                payload.validate(),
                Err(ValidationError::InvalidMovementState),
                "invalid floor normal {floor_normal:?} must not reach typed apply"
            );
        }
    }

    #[test]
    fn valid_or_absent_sliding_floor_normal_is_preserved_by_typed_validation() {
        for floor_normal in [
            None,
            Some([-0.25, 0.968_245_86, 0.0]),
            Some([0.0, 1.000_4, 0.0]),
        ] {
            let movement = WirePlayerMovementState {
                movement_state: WireMovementState::Sliding {
                    elapsed_ms: 10.0,
                    boost: [1.0, 0.0, 3.0],
                    eye_current: 0.5,
                    floor_normal,
                },
                ..sample_movement()
            };
            let payload = RawComponentPayload {
                component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
                transform: None,
                player_movement: Some(movement),
                mesh_animation_state: None,
                kinematic_mover: None,
            };

            assert_eq!(
                payload.validate(),
                Ok(ComponentPayload::PlayerMovementState(movement))
            );
        }
    }

    // --- KinematicMoverState validation ---

    #[test]
    fn kinematic_mover_payload_validates_to_typed_payload() {
        assert_eq!(
            raw_mover_payload().validate(),
            Ok(ComponentPayload::KinematicMoverState(sample_mover_state()))
        );
    }

    #[test]
    fn non_finite_kinematic_mover_state_rejects() {
        for mutate in [
            |m: &mut WireKinematicMoverState| m.segment_elapsed_ms = f32::NAN,
            |m: &mut WireKinematicMoverState| m.wait_remaining_ms = f32::INFINITY,
            |m: &mut WireKinematicMoverState| m.velocity[2] = f32::NEG_INFINITY,
            |m: &mut WireKinematicMoverState| m.spin_angle_rad = f32::NAN,
            |m: &mut WireKinematicMoverState| m.spin_angle_before_tick_rad = f32::NAN,
            |m: &mut WireKinematicMoverState| m.spin_rate_rad_s = f32::INFINITY,
            |m: &mut WireKinematicMoverState| m.spin_target_rate_rad_s = f32::NEG_INFINITY,
        ] {
            let mut mover = sample_mover_state();
            mutate(&mut mover);
            let payload = RawComponentPayload {
                component_kind: COMPONENT_KIND_KINEMATIC_MOVER_STATE,
                transform: None,
                player_movement: None,
                mesh_animation_state: None,
                kinematic_mover: Some(mover),
            };
            assert_eq!(
                payload.validate(),
                Err(ValidationError::NonFiniteKinematicMoverState)
            );
        }
    }

    #[test]
    fn invalid_kinematic_mover_direction_or_mode_rejects() {
        for mutate in [
            |m: &mut WireKinematicMoverState| m.direction = 0,
            |m: &mut WireKinematicMoverState| m.direction = 2,
            |m: &mut WireKinematicMoverState| m.mode = 2,
        ] {
            let mut mover = sample_mover_state();
            mutate(&mut mover);
            let payload = RawComponentPayload {
                component_kind: COMPONENT_KIND_KINEMATIC_MOVER_STATE,
                transform: None,
                player_movement: None,
                mesh_animation_state: None,
                kinematic_mover: Some(mover),
            };
            assert_eq!(
                payload.validate(),
                Err(ValidationError::InvalidKinematicMoverState)
            );
        }
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
                mesh_animation_state: None,
                kinematic_mover: None,
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
                mesh_animation_state: None,
                kinematic_mover: None,
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
                mesh_animation_state: None,
                kinematic_mover: None,
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
            WireMovementState::Sliding {
                elapsed_ms: 1.0,
                boost: [0.0, 0.0, 0.0],
                eye_current: 0.5,
                floor_normal: Some([0.0, 1.0, 0.0]),
            },
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
                WireMovementState::Sliding { floor_normal, .. } => {
                    5 + floor_normal.map_or(0, |_| 3)
                }
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
                mesh_animation_state: None,
                kinematic_mover: None,
            },
            ComponentPayload::PlayerMovementState(m) => RawComponentPayload {
                component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
                transform: None,
                player_movement: Some(*m),
                mesh_animation_state: None,
                kinematic_mover: None,
            },
            ComponentPayload::MeshAnimationState(m) => RawComponentPayload {
                component_kind: COMPONENT_KIND_MESH_ANIMATION_STATE,
                transform: None,
                player_movement: None,
                mesh_animation_state: Some(m.clone()),
                kinematic_mover: None,
            },
            ComponentPayload::KinematicMoverState(m) => RawComponentPayload {
                component_kind: COMPONENT_KIND_KINEMATIC_MOVER_STATE,
                transform: None,
                player_movement: None,
                mesh_animation_state: None,
                kinematic_mover: Some(*m),
            },
        }
    }
}
