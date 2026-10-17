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
// `SimCommand` <-> `InputCommand`) live in `postretro-netcode`, not
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

mod components;
mod teleport;
pub use components::{
    COMPONENT_KIND_AGENT, COMPONENT_KIND_AMMO_RESERVE, COMPONENT_KIND_BILLBOARD_EMITTER,
    COMPONENT_KIND_BRAIN, COMPONENT_KIND_DEFERRED_EFFECT, COMPONENT_KIND_DESCRIPTOR_PROVENANCE,
    COMPONENT_KIND_ENTITY_STATE, COMPONENT_KIND_FOG_VOLUME, COMPONENT_KIND_HEALTH,
    COMPONENT_KIND_HIT_VOLUME, COMPONENT_KIND_IMMERSION, COMPONENT_KIND_INVENTORY,
    COMPONENT_KIND_KINEMATIC_MOVER_STATE, COMPONENT_KIND_LIGHT, COMPONENT_KIND_MAP_ORDINAL,
    COMPONENT_KIND_MESH_ANIMATION_STATE, COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
    COMPONENT_KIND_PROJECTILE, COMPONENT_KIND_SPAWNER, COMPONENT_KIND_SPRITE_VISUAL,
    COMPONENT_KIND_TELEPORTER, COMPONENT_KIND_TOUCHABLE, COMPONENT_KIND_TRANSFORM,
    COMPONENT_KIND_TRIGGER_VOLUME, COMPONENT_KIND_WEAPON, ComponentPayload, MAX_WIRE_CURVE_KEYS,
    RawComponentPayload, WireEmitterState, WireFogAnimation, WireFogState, WireGroundRef,
    WireKinematicMoverState, WireLightAnimation, WireLightState, WireMeshAnimationState,
    WireMovementState, WirePlayerMovementState, WireSpinAnimation, WireTransform,
};
pub use teleport::{
    NO_WIRE_TELEPORTER, WireTeleportState, WireTeleporterState, teleport_yaw_is_valid,
};

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

/// One host-to-client presentation message: a passive presentation event, or a
/// host presentation command routed to this client by audience.
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
/// bitcode encodes enum tags positionally. Wire 28 replaces the attacker-private
/// OverlayFact at tag 1 with complete shared roster state; older peers are gated
/// before decode. Transient Spawn and Command retain their existing semantics.
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
    /// Complete bounded live roster. Absence at a newer revision retires a bar;
    /// repair sends repeat current state without replaying combat events.
    OverlayRoster(WireOverlayRoster),
    /// A host presentation command routed to this client by audience. The client
    /// turns it into the same local command a local reaction enqueues.
    Command(PresentationCommand),
}

/// Mirrors the existing authoring limit, checked against it in netcode tests.
pub const MAX_OVERLAY_ROSTER_ENTRIES: usize = 64;

/// One live enemy in the shared host-selected overlay roster. Network ids are
/// never recycled; a newer complete roster supplies removal/terminal ordering.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireEnemyOverlay {
    pub enemy_id: NetworkId,
    pub health_fraction: f32,
    pub shield_fraction: f32,
    pub has_shield: bool,
    pub damaged_at: f64,
    pub expires_at: f64,
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WireOverlayRoster {
    pub revision: u64,
    /// Host script clock at publication, not at reception or original damage.
    pub server_script_time: f64,
    /// Sorted by stable NetworkId; every entry is live at publication.
    pub entries: Vec<WireEnemyOverlay>,
}

impl WireOverlayRoster {
    pub fn is_valid(&self) -> bool {
        self.revision > 0
            && self.server_script_time.is_finite()
            && self.server_script_time >= 0.0
            && self.entries.len() <= MAX_OVERLAY_ROSTER_ENTRIES
            && self
                .entries
                .windows(2)
                .all(|pair| pair[0].enemy_id.0 < pair[1].enemy_id.0)
            && self.entries.iter().all(|entry| {
                entry.health_fraction.is_finite()
                    && entry.health_fraction > 0.0
                    && entry.shield_fraction.is_finite()
                    && entry.shield_fraction >= 0.0
                    && entry.damaged_at.is_finite()
                    && entry.damaged_at >= 0.0
                    && entry.damaged_at <= self.server_script_time
                    && entry.expires_at.is_finite()
                    && entry.expires_at > entry.damaged_at
                    && entry.expires_at > self.server_script_time
            })
    }
}

/// Wire mirror of the engine's presentation system-reaction commands.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum PresentationCommand {
    PlaySound {
        sound: String,
        bus: Option<String>,
        at: Option<[f32; 3]>,
    },
    Rumble {
        strong: f32,
        weak: Option<f32>,
        duration_ms: f32,
    },
    FlashScreen {
        color: [f32; 4],
        duration_ms: f32,
    },
    Vignette {
        color: Option<[f32; 3]>,
        strength: f32,
        duration_ms: f32,
    },
    ScreenShake {
        amplitude: f32,
        duration_ms: f32,
        frequency: Option<f32>,
    },
}

/// Frozen scalar facts for a descriptor-backed observer projectile. Assets stay local.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireProjectilePresentation {
    pub sprite_size: Option<f32>,
    pub model_scale: f32,
    pub shot_id: Option<WireShotId>,
}

impl WireProjectilePresentation {
    pub fn is_valid(&self) -> bool {
        self.model_scale.is_finite()
            && self.model_scale > 0.0
            && self
                .sprite_size
                .is_none_or(|size| size.is_finite() && size > 0.0)
            && self.shot_id.is_none_or(valid_presentation_shot_id)
    }
}

pub const MAX_WEAPON_CUES: usize = 64;
/// Matches the authored hitscan pellet bound; aggregation remains per shot.
pub const MAX_WEAPON_CUE_CONTACTS: usize = 32;
/// Mirrored authored cue bound; netcode asserts compatibility without coupling floors.
pub const MAX_WEAPON_CUE_NAME_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub enum WeaponCueKind {
    Activate,
    Impact,
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WeaponCueContact {
    pub point: [f32; 3],
    pub normal: [f32; 3],
    pub target: Option<NetworkId>,
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum WeaponCueAnchor {
    Entity { entity: NetworkId, origin: [f32; 3] },
    Contacts(Vec<WeaponCueContact>),
}

/// The producer freezes sound keys and aliases before descriptor replacement/despawn.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WeaponCue {
    pub shot_id: WireShotId,
    pub kind: WeaponCueKind,
    pub sound: Option<String>,
    pub additional_sound: Option<String>,
    pub alias: Option<String>,
    pub anchor: WeaponCueAnchor,
}

impl WeaponCue {
    pub fn is_valid(&self) -> bool {
        let name = |value: &Option<String>| {
            value.as_ref().is_none_or(|value| {
                !value.trim().is_empty() && value.len() <= MAX_WEAPON_CUE_NAME_BYTES
            })
        };
        let builtin = match self.kind {
            WeaponCueKind::Activate => "activate",
            WeaponCueKind::Impact => "impact",
        };
        valid_presentation_shot_id(self.shot_id)
            && name(&self.sound)
            && name(&self.additional_sound)
            && name(&self.alias)
            && self.alias.as_deref() != Some(builtin)
            && match &self.anchor {
                WeaponCueAnchor::Entity { origin, .. } => origin.iter().all(|v| v.is_finite()),
                WeaponCueAnchor::Contacts(contacts) => {
                    !contacts.is_empty()
                        && contacts.len() <= MAX_WEAPON_CUE_CONTACTS
                        && contacts.iter().all(|contact| {
                            contact
                                .point
                                .iter()
                                .chain(&contact.normal)
                                .all(|v| v.is_finite())
                        })
                }
            }
    }
}

fn valid_presentation_shot_id(id: WireShotId) -> bool {
    id.lane <= 1 && id.ordinal < 16
}

/// Reliable cues are participation-framed independently of owner-private Input facts.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WeaponCuesMessage {
    pub participation_epoch: u64,
    pub cues: Vec<WeaponCue>,
}

impl WeaponCuesMessage {
    pub fn is_valid(&self) -> bool {
        self.cues.len() <= MAX_WEAPON_CUES && self.cues.iter().all(WeaponCue::is_valid)
    }
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
///
/// Bumped to 15 for the faction-sentiment sparse sync record. Its complete overlay
/// set rides the snapshot envelope and has its own baseline/delta state machine.
///
/// Bumped to 16 for the protected knockback velocity in player movement state.
/// Bumped to 17 for frozen projectile body size, model scale, and shot provenance.
/// Bumped to 19 for the protected push-drift velocity in player movement state;
/// `WireMovementState::Swim` and the swim re-entry latch share that bump.
/// Bumped to 20 for `WireMovementState::Vault`.
/// Bumped to 21 for teleporters: player movement state gained the pawn teleport
/// record, and entity records gained the teleporter armed-state payload.
pub const SNAPSHOT_VERSION: u16 = 21;

/// Stable map presentation identity. The receiver verifies the origin before applying.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WirePresentationBinding {
    /// 0 = authored map light, 1 = fog volume, 2 = map entity ordinal.
    pub kind: u8,
    pub index: u32,
    pub origin: [f32; 3],
}

/// `record_kind` discriminant for a full-baseline (spawn / join / refresh) record.
pub const RECORD_KIND_FULL_BASELINE: u16 = 0;
/// `record_kind` discriminant for a delta-update record.
pub const RECORD_KIND_DELTA: u16 = 1;
/// `record_kind` discriminant for a despawn record.
pub const RECORD_KIND_DESPAWN: u16 = 2;

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
    pub projectile_presentation: Option<WireProjectilePresentation>,
    pub presentation_binding: Option<WirePresentationBinding>,
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
    /// Host-authoritative sparse faction-sentiment overlay update. This is separate
    /// from `state_records`: it is a dynamic set, not a static descriptor-backed
    /// slot. `None` means this snapshot carries no new faction-sentiment baseline.
    pub faction_sentiment_record: Option<crate::state_slots::RawFactionSentimentRecord>,
    pub server_script_time: f64,
}

// ---------------------------------------------------------------------------
// Typed apply model (produced only after validation)
// ---------------------------------------------------------------------------

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
        projectile_presentation: Option<WireProjectilePresentation>,
        presentation_binding: Option<WirePresentationBinding>,
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
        projectile_presentation: Option<WireProjectilePresentation>,
        presentation_binding: Option<WirePresentationBinding>,
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
    /// Raw sparse faction-sentiment overlay record. It is validated by the engine
    /// alongside the installed faction registry before the overlay is replaced.
    pub faction_sentiment_record: Option<crate::state_slots::RawFactionSentimentRecord>,
    pub server_script_time: f64,
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
    VersionMismatch {
        expected: u16,
        received: u16,
    },
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
    MalformedTickMetadata {
        last_processed_client_tick: u32,
    },
    /// A despawn record carried metadata. Tombstones carry no movement authority,
    /// descriptor/weapon metadata, or projectile/presentation metadata.
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
    /// must be safely bounded and near-unit. A Vault payload must have a
    /// non-negative timer and entry speed, a bounded target and start offset,
    /// and a start offset with a horizontal component.
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
    /// A teleporter armed-state payload named the no-teleporter sentinel index.
    /// Whether the index names an installed teleporter is engine-side binding.
    InvalidTeleporterState,
    InvalidProjectilePresentation,
    NonFinitePresentationState(u16),
    InvalidPresentationState(u16),
    InvalidPresentationBinding,
    PresentationStateWithoutTransform,
    NonFiniteScriptTime,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::NonFinitePresentationState(kind) => {
                write!(f, "non-finite presentation state for component kind {kind}")
            }
            ValidationError::InvalidPresentationState(kind) => write!(
                f,
                "invalid presentation animation for component kind {kind}"
            ),
            ValidationError::InvalidPresentationBinding => {
                write!(f, "invalid presentation binding or bound record shape")
            }
            ValidationError::PresentationStateWithoutTransform => {
                write!(f, "unbound presentation state requires a Transform")
            }
            ValidationError::NonFiniteScriptTime => write!(f, "non-finite server script time"),
            ValidationError::InvalidTeleporterState => {
                write!(f, "teleporter state names the no-teleporter index")
            }
            ValidationError::VersionMismatch { expected, received } => write!(
                f,
                "snapshot version mismatch: expected {expected}, received {received}"
            ),
            ValidationError::InvalidProjectilePresentation => write!(
                f,
                "invalid projectile presentation facts or missing finite descriptor pose"
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
                write!(f, "forbidden metadata on a despawn record")
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
                    "invalid PlayerMovementState contract: a Sliding boost or floor normal, or a Vault payload, is out of contract"
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
                self.validate_projectile_presentation(&components, entity_class.as_deref())?;
                self.validate_presentation_binding(&components, entity_class.as_deref())?;
                Ok(EntityRecord::FullBaseline {
                    network_id: self.network_id,
                    baseline_id: self.baseline_id_or_ref,
                    last_processed_client_tick,
                    local_player: self.local_player,
                    entity_class,
                    active_weapon_archetype,
                    components,
                    projectile_presentation: self.projectile_presentation,
                    presentation_binding: self.presentation_binding,
                })
            }
            RECORD_KIND_DELTA => {
                let components = self.validate_components()?;
                let last_processed_client_tick = self.validate_movement_metadata(&components)?;
                let entity_class = self.validate_entity_class(&components)?;
                let active_weapon_archetype = self.validate_active_weapon_metadata(&components)?;
                self.validate_projectile_presentation(&components, entity_class.as_deref())?;
                self.validate_presentation_binding(&components, entity_class.as_deref())?;
                Ok(EntityRecord::Delta {
                    network_id: self.network_id,
                    baseline_ref: self.baseline_id_or_ref,
                    new_baseline_id: self.new_baseline_id_or_tombstone_id,
                    last_processed_client_tick,
                    local_player: self.local_player,
                    entity_class,
                    active_weapon_archetype,
                    components,
                    projectile_presentation: self.projectile_presentation,
                    presentation_binding: self.presentation_binding,
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
                    || self.projectile_presentation.is_some()
                    || self.presentation_binding.is_some()
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

    fn validate_presentation_binding(
        &self,
        components: &[ComponentPayload],
        entity_class: Option<&str>,
    ) -> Result<(), ValidationError> {
        let has_transform = components
            .iter()
            .any(|p| matches!(p, ComponentPayload::Transform(_)));
        let has_state = components.iter().any(|p| {
            matches!(
                p,
                ComponentPayload::LightState(_)
                    | ComponentPayload::FogState(_)
                    | ComponentPayload::EmitterState(_)
            )
        });
        if let Some(binding) = self.presentation_binding {
            if binding.kind > 2
                || !binding.origin.iter().all(|v| v.is_finite())
                || entity_class.is_some()
                || has_transform
                || !has_state
            {
                return Err(ValidationError::InvalidPresentationBinding);
            }
        } else if has_state && !has_transform {
            return Err(ValidationError::PresentationStateWithoutTransform);
        }
        Ok(())
    }

    fn validate_projectile_presentation(
        &self,
        components: &[ComponentPayload],
        entity_class: Option<&str>,
    ) -> Result<(), ValidationError> {
        if let Some(facts) = self.projectile_presentation
            && (!facts.is_valid()
                || entity_class.is_none()
                || !components
                    .iter()
                    .any(|p| matches!(p, ComponentPayload::Transform(t) if t.all_finite())))
        {
            return Err(ValidationError::InvalidProjectilePresentation);
        }
        Ok(())
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
                ComponentPayload::PlayerMovementState(_) | ComponentPayload::TeleporterState(_) => {
                    false
                }
                ComponentPayload::MeshAnimationState(_) => false,
                ComponentPayload::KinematicMoverState(_)
                | ComponentPayload::LightState(_)
                | ComponentPayload::FogState(_)
                | ComponentPayload::EmitterState(_) => false,
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
        if !self.server_script_time.is_finite() {
            return Err(ValidationError::NonFiniteScriptTime);
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
            faction_sentiment_record: self.faction_sentiment_record.clone(),
            server_script_time: self.server_script_time,
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
    /// The accumulated teleport yaw the client held when it sampled this
    /// command's facing. Host resolution and client replay turn facing, and the
    /// intent relative to it, by the pawn's current accumulated yaw minus this
    /// stamp. Same wrapped representation as the pawn teleport record.
    pub teleport_yaw_stamp: f32,
    pub movement: WireMovementInput,
    pub fire_button: WireFireButtonState,
    pub reload: bool,
    pub secondary_button: WireFireButtonState,
    pub activation: WireActivationInput,
}

/// Explicit activation identity mirrors; unknown lane/ordinal tags reject at intake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Encode, Decode)]
pub struct WireActivationToken {
    pub start_tick: u32,
    pub lane: u8,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Encode, Decode)]
pub struct WireShotId {
    pub pawn: u32,
    pub start_tick: u32,
    pub lane: u8,
    pub ordinal: u8,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct WireActivationRelease {
    pub token: WireActivationToken,
    pub release_tick: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub struct WireActivationInput {
    pub initiation: Option<WireActivationToken>,
    pub release: Option<WireActivationRelease>,
    pub cancel: Option<WireActivationToken>,
}
/// Reliable owner-private activation lifecycle. Recovery is whole fixed ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub enum ActivationOutcome {
    InitiationAccepted {
        token: WireActivationToken,
        weapon: NetworkId,
    },
    InitiationRejected {
        token: WireActivationToken,
        recovery_ticks: u32,
    },
    ExecutionAccepted {
        token: WireActivationToken,
        weapon: NetworkId,
        charge_millionths: u32,
        recovery_ticks: u32,
    },
    Cancelled {
        token: WireActivationToken,
        weapon: NetworkId,
        recovery_ticks: u32,
    },
    Completed {
        token: WireActivationToken,
        weapon: NetworkId,
        recovery_ticks: u32,
    },
}

/// One client-declared hit record for a host-authorized shot. `target` is normally
/// a `NetworkId` (`u32`) because the net crate is registry-blind. `u32::MAX` is a
/// presentation-only contact marker for a world contact with no damage target.
/// Hitscan drops an entity hit it cannot name on the wire rather than declaring
/// it; projectile resolution instead falls back to the world-contact sentinel
/// when its target is no longer nameable. `normal` is the surface normal at
/// `point`, carried so the host holds the same contact data the client resolved.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct HitRecord {
    pub target: u32,
    pub point: [f32; 3],
    pub zone: Option<String>,
    pub normal: [f32; 3],
}

/// Standalone client -> server hit declaration. It intentionally does not ride
/// [`InputCommand`]: a resolved hit may arrive later than the FIRE command that
/// authorized `shot_id` (projectile-ready shape).
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct HitDeclaration {
    pub shot_id: WireShotId,
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
    /// Latest complete sparse faction-sentiment baseline the client applied. This is
    /// separate from static state-slot ids because the overlay has no descriptor.
    /// `None` leaves the server's known baseline unchanged.
    pub faction_sentiment_baseline: Option<u32>,
}

/// Client -> server request to re-send a full baseline for one entity, carried on
/// the reliable-ordered `Channel::Input`. Sent when a `Delta` arrives for an entity
/// the client holds no baseline or live mapping for, or whose new baseline id is
/// older than the one it holds; also for the other repair causes `reason` names.
/// The server responds with a `FullBaseline` record for that entity on
/// `Channel::Snapshot`.
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
    /// The entity to re-send a full baseline for.
    pub network_id: u32,
    /// The `baseline_ref` the unappliable delta named. Diagnostic and dedup
    /// context only: a delta applies whatever reference it names, so this need not
    /// be a baseline the client lacks.
    pub missing_baseline_ref: u32,
    /// Why the refresh is needed (e.g. unknown baseline). A `u8` reason code,
    /// not interpreted by the repair path — logged for diagnostics.
    pub reason: u8,
}

/// Client -> server request to re-send a full baseline for one replicated *state
/// slot*, carried on the reliable-ordered `Channel::Input` (M15 Phase 3.5).
///
/// Distinct from [`BaselineRefreshRequest`] by design: entity baselines are keyed
/// by `NetworkId`, while state baselines are keyed by `StateSlotId`. Sent when a
/// state `Delta` arrives for a slot the client holds no baseline for, or whose new
/// baseline id is older than the one it holds; the server schedules a
/// `FullBaseline` for that slot on `Channel::Snapshot`.
/// Requests are additive and deduped server-side by `(client, slot_id,
/// missing_baseline_ref)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct StateBaselineRefreshRequest {
    /// The snapshot `sequence` whose state delta could not be applied. Diagnostic /
    /// dedup context; the repair is keyed by slot + missing ref, not sequence.
    pub snapshot_sequence: u32,
    /// The replicated state slot to re-send a full baseline for (`StateSlotId`
    /// inner value).
    pub slot_id: u16,
    /// The `baseline_ref` the unappliable state delta named. Diagnostic and dedup
    /// context only: a state delta applies whatever reference it names.
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
    pub shot_id: WireShotId,
    /// Whether the host authorized the FIRE that minted this shot. `false` means no
    /// host-authorized shot existed, so clients retract muzzle/flight. Recovery
    /// correction comes from the correlated activation outcome.
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
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ServerMessage {
    TimeSync(crate::timesync::TimeSyncEcho),
    ShotVerdicts(ShotVerdictsMessage),
    ActivationOutcomes(Vec<ActivationOutcome>),
    WeaponCues(WeaponCuesMessage),
    /// A HIT declaration exceeded its owner's pending limit or expired while
    /// FIRE was undecided. Settles HIT bookkeeping without deciding FIRE.
    HitRefused(WireShotId),
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
            teleport: Default::default(),
            knockback_velocity: [0.0; 3],
            push_velocity: [0.0; 3],
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
            swim_reentry_latched: false,
            capsule_half_height: 0.8,
            capsule_eye_height: 1.5,
            aim_pitch: -0.45,
        }
    }

    fn raw_transform_payload() -> RawComponentPayload {
        RawComponentPayload {
            teleporter: None,
            component_kind: COMPONENT_KIND_TRANSFORM,
            transform: Some(sample_transform()),
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
        }
    }

    fn raw_movement_payload() -> RawComponentPayload {
        RawComponentPayload {
            teleporter: None,
            component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            transform: None,
            player_movement: Some(sample_movement()),
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
        }
    }

    fn raw_mesh_animation_payload(state: &str) -> RawComponentPayload {
        RawComponentPayload {
            teleporter: None,
            component_kind: COMPONENT_KIND_MESH_ANIMATION_STATE,
            transform: None,
            player_movement: None,
            mesh_animation_state: Some(WireMeshAnimationState {
                current_state: state.to_string(),
            }),
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
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
            teleporter: None,
            component_kind: COMPONENT_KIND_KINEMATIC_MOVER_STATE,
            transform: None,
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: Some(sample_mover_state()),
            light: None,
            emitter: None,
            fog: None,
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
            projectile_presentation: None,
            presentation_binding: None,
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
            server_script_time: 0.0,
            version: SNAPSHOT_VERSION,
            sequence,
            server_tick,
            records,
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
            faction_sentiment_record: None,
        }
    }

    fn sample_input() -> InputCommand {
        InputCommand {
            teleport_yaw_stamp: 0.0,
            secondary_button: WireFireButtonState {
                pressed: false,
                active: false,
            },
            activation: WireActivationInput::default(),
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
            FACTION_SENTIMENT_RECORD_KIND_FULL_BASELINE, FactionSentimentPair,
            RawFactionSentimentRecord, RawStateSlotRecord, STATE_RECORD_KIND_FULL_BASELINE,
            WireSlotValue,
        };
        let raw = RawSnapshotMessage {
            server_script_time: 0.0,
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
            faction_sentiment_record: Some(RawFactionSentimentRecord {
                kind: FACTION_SENTIMENT_RECORD_KIND_FULL_BASELINE,
                has_baseline_ref: false,
                baseline_ref: 0,
                baseline_id: 11,
                pairs: vec![FactionSentimentPair {
                    from_idx: 2,
                    to_idx: 3,
                    value: -0.75,
                }],
            }),
        };
        assert!(round_trips(&raw));
        // The state fields survive a decode and reach the typed apply model.
        let bytes = encode(&raw);
        let decoded: RawSnapshotMessage = decode(&bytes).expect("snapshot decodes");
        let typed = decoded.validate().expect("entity half validates");
        assert_eq!(typed.state_schema_fingerprint, [9u8; 32]);
        assert_eq!(typed.state_records.len(), 1);
        assert!(typed.faction_sentiment_record.is_some());
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
            WireMovementState::Swim { fluid: 2 },
        ] {
            for swim_reentry_latched in [false, true] {
                let movement = WirePlayerMovementState {
                    teleport: Default::default(),
                    movement_state: state,
                    swim_reentry_latched,
                    ..sample_movement()
                };
                assert!(round_trips(&movement));
            }
        }
    }

    // A swimmer's drift layer carries vertical push; all three axes cross the
    // wire.
    #[test]
    fn swimming_movement_state_round_trips_its_vertical_drift() {
        let movement = WirePlayerMovementState {
            teleport: Default::default(),
            movement_state: WireMovementState::Swim { fluid: 0 },
            push_velocity: [1.5, -2.25, 0.5],
            ..sample_movement()
        };
        assert!(round_trips(&movement));
        let bytes = encode(&movement);
        let decoded: WirePlayerMovementState = decode(&bytes).expect("decodes");
        assert_eq!(decoded.push_velocity, [1.5, -2.25, 0.5]);
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
            shot_id: WireShotId {
                pawn: 1,
                start_tick: 9,
                lane: 0,
                ordinal: 0,
            },
            records: Vec::new(),
        };
        assert!(round_trips(&empty));

        let declaration = HitDeclaration {
            shot_id: WireShotId {
                pawn: 1,
                start_tick: 0x2345_6789,
                lane: 0,
                ordinal: 0,
            },
            records: vec![
                HitRecord {
                    normal: [0.0, 1.0, 0.0],
                    target: 17,
                    point: [1.0, 2.5, -3.0],
                    zone: Some("head".to_string()),
                },
                HitRecord {
                    normal: [0.0, 1.0, 0.0],
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
                    shot_id: WireShotId {
                        pawn: 1,
                        start_tick: 10,
                        lane: 0,
                        ordinal: 0,
                    },
                    accept: true,
                    hit_accepted: true,
                },
                ShotVerdict {
                    shot_id: WireShotId {
                        pawn: 1,
                        start_tick: 11,
                        lane: 0,
                        ordinal: 0,
                    },
                    accept: false,
                    hit_accepted: false,
                },
            ],
        });
        assert!(round_trips(&verdicts));
    }

    #[test]
    fn hit_refusal_server_message_preserves_identity_and_rejects_truncation() {
        let message = ServerMessage::HitRefused(WireShotId {
            pawn: u32::MAX,
            start_tick: u32::MAX - 1,
            lane: 1,
            ordinal: 15,
        });
        let bytes = encode(&message);
        assert_eq!(decode::<ServerMessage>(&bytes).unwrap(), message);
        assert!(matches!(
            decode::<ServerMessage>(&bytes[..bytes.len() - 1]),
            Err(WireError::Decode(_))
        ));
    }

    #[test]
    fn ack_message_round_trips() {
        let ack = AckMessage {
            latest_snapshot_sequence: 17,
            acked_server_tick: 510,
            entity_baselines: vec![(3, 9), (7, 2), (42, 100)],
            despawn_tombstones: vec![(11, 4)],
            slot_baselines: vec![(1, 7), (2, 3)],
            faction_sentiment_baseline: Some(13),
        };
        assert!(round_trips(&ack));
        // An empty ack (no per-entity progress) is still a valid carrier.
        let empty = AckMessage {
            latest_snapshot_sequence: 0,
            acked_server_tick: 0,
            entity_baselines: Vec::new(),
            despawn_tombstones: Vec::new(),
            slot_baselines: Vec::new(),
            faction_sentiment_baseline: None,
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
                faction_sentiment_baseline: Some(8),
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
                shot_id: WireShotId {
                    pawn: 1,
                    start_tick: 99,
                    lane: 0,
                    ordinal: 0,
                },
                records: vec![HitRecord {
                    normal: [0.0, 1.0, 0.0],
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
                    shot_id: WireShotId {
                        pawn: 1,
                        start_tick: 8,
                        lane: 0,
                        ordinal: 0,
                    },
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
        const {
            assert!(
                SNAPSHOT_VERSION >= 18,
                "presentation state and clock require snapshot version 18"
            );
        }

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
            payload: ServerPresentationPayload::OverlayRoster(WireOverlayRoster {
                revision: 4,
                server_script_time: 10.0,
                entries: vec![WireEnemyOverlay {
                    enemy_id: NetworkId(77),
                    health_fraction: 0.25,
                    shield_fraction: 0.5,
                    has_shield: true,
                    damaged_at: 9.0,
                    expires_at: 12.0,
                }],
            }),
        };
        assert!(round_trips(&overlay));
    }

    #[test]
    fn overlay_roster_validates_capacity_order_finite_facts_and_absolute_expiry() {
        let entry = WireEnemyOverlay {
            enemy_id: NetworkId(0),
            health_fraction: 0.5,
            shield_fraction: 0.0,
            has_shield: false,
            damaged_at: 1.0,
            expires_at: 3.0,
        };
        let valid = WireOverlayRoster {
            revision: 1,
            server_script_time: 2.0,
            entries: vec![entry],
        };
        assert!(valid.is_valid(), "the first allocated NetworkId is zero");
        let mut roster = valid.clone();
        roster.entries = (0..MAX_OVERLAY_ROSTER_ENTRIES)
            .map(|id| WireEnemyOverlay {
                enemy_id: NetworkId(id as u32),
                ..entry
            })
            .collect();
        assert!(roster.is_valid());
        assert!(round_trips(&roster));
        roster.entries.push(WireEnemyOverlay {
            enemy_id: NetworkId(MAX_OVERLAY_ROSTER_ENTRIES as u32),
            ..entry
        });
        assert!(!roster.is_valid());
        for bad in [
            WireEnemyOverlay {
                health_fraction: f32::NAN,
                ..entry
            },
            WireEnemyOverlay {
                health_fraction: 0.0,
                ..entry
            },
            WireEnemyOverlay {
                shield_fraction: -1.0,
                ..entry
            },
            WireEnemyOverlay {
                shield_fraction: f32::INFINITY,
                ..entry
            },
            WireEnemyOverlay {
                damaged_at: -1.0,
                ..entry
            },
            WireEnemyOverlay {
                damaged_at: 2.1,
                ..entry
            },
            WireEnemyOverlay {
                expires_at: 2.0,
                ..entry
            },
            WireEnemyOverlay {
                expires_at: f64::INFINITY,
                ..entry
            },
        ] {
            assert!(
                !WireOverlayRoster {
                    entries: vec![bad],
                    ..valid.clone()
                }
                .is_valid()
            );
        }
        assert!(
            !WireOverlayRoster {
                revision: 0,
                ..valid.clone()
            }
            .is_valid()
        );
        assert!(
            !WireOverlayRoster {
                server_script_time: f64::NAN,
                ..valid.clone()
            }
            .is_valid()
        );
        assert!(
            !WireOverlayRoster {
                entries: vec![entry, entry],
                ..valid.clone()
            }
            .is_valid()
        );
        assert!(
            !WireOverlayRoster {
                entries: vec![
                    WireEnemyOverlay {
                        enemy_id: NetworkId(1),
                        ..entry
                    },
                    entry
                ],
                ..valid
            }
            .is_valid()
        );
    }

    #[test]
    fn every_presentation_command_round_trips_with_identical_fields() {
        let commands = [
            PresentationCommand::PlaySound {
                sound: "level_up".to_string(),
                bus: Some("ui".to_string()),
                at: Some([1.0, 2.0, 3.0]),
            },
            PresentationCommand::PlaySound {
                sound: "scald".to_string(),
                bus: None,
                at: None,
            },
            PresentationCommand::Rumble {
                strong: 0.75,
                weak: Some(0.25),
                duration_ms: 120.0,
            },
            PresentationCommand::FlashScreen {
                color: [1.0, 0.9, 0.3, 0.4],
                duration_ms: 300.0,
            },
            PresentationCommand::Vignette {
                color: Some([0.8, 0.0, 0.0]),
                strength: 0.6,
                duration_ms: 800.0,
            },
            PresentationCommand::ScreenShake {
                amplitude: 0.4,
                duration_ms: 250.0,
                frequency: Some(18.0),
            },
        ];
        for command in commands {
            let message = ServerPresentationMessage {
                payload: ServerPresentationPayload::Command(command),
            };
            assert!(round_trips(&message), "{message:?}");
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
            server_script_time: 0.0,
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
            faction_sentiment_record: None,
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
                projectile_presentation: None,
                presentation_binding: None,
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
            server_script_time: 0.0,
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
            faction_sentiment_record: None,
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
                projectile_presentation: None,
                presentation_binding: None,
                components: vec![ComponentPayload::Transform(sample_transform())],
            }]
        );
    }

    #[test]
    fn validate_despawn_maps_tombstone_and_reason() {
        let raw = RawSnapshotMessage {
            server_script_time: 0.0,
            version: SNAPSHOT_VERSION,
            sequence: 6,
            server_tick: 62,
            records: vec![raw_record(RECORD_KIND_DESPAWN, 9, 0, 42, 7, Vec::new())],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
            faction_sentiment_record: None,
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
            server_script_time: 0.0,
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
            faction_sentiment_record: None,
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
            server_script_time: 0.0,
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
            faction_sentiment_record: None,
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
            server_script_time: 0.0,
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
                    teleporter: None,
                    component_kind: 1234, // not Transform/PlayerMovementState
                    transform: Some(sample_transform()),
                    player_movement: None,
                    mesh_animation_state: None,
                    kinematic_mover: None,
                    light: None,
                    emitter: None,
                    fog: None,
                }],
            )],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
            faction_sentiment_record: None,
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
            teleporter: None,
            component_kind: COMPONENT_KIND_TRANSFORM,
            transform: None, // kind says Transform but slot is empty
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
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
            teleporter: None,
            component_kind: COMPONENT_KIND_TRANSFORM,
            transform: Some(sample_transform()),
            player_movement: Some(sample_movement()),
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
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
            teleporter: None,
            component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            transform: Some(sample_transform()),
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
        };
        assert_eq!(
            payload.validate(),
            Err(ValidationError::MissingComponentPayload(
                COMPONENT_KIND_PLAYER_MOVEMENT_STATE
            ))
        );
    }

    #[test]
    fn knockback_snapshot_version_rejects_immediately_previous_layout() {
        const PRE_KNOCKBACK_SNAPSHOT_VERSION: u16 = 15;
        const {
            assert!(
                SNAPSHOT_VERSION > PRE_KNOCKBACK_SNAPSHOT_VERSION,
                "later layouts retain the protected knockback field"
            );
        }
        let raw = RawSnapshotMessage {
            server_script_time: 0.0,
            version: PRE_KNOCKBACK_SNAPSHOT_VERSION,
            sequence: 1,
            server_tick: 1,
            records: Vec::new(),
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
            faction_sentiment_record: None,
        };
        assert_eq!(
            raw.validate(),
            Err(ValidationError::VersionMismatch {
                expected: SNAPSHOT_VERSION,
                received: PRE_KNOCKBACK_SNAPSHOT_VERSION,
            })
        );
    }

    #[test]
    fn first_bad_record_short_circuits_validation() {
        // A good record followed by a bad one: validation rejects the whole
        // snapshot and produces no partial typed result.
        let raw = RawSnapshotMessage {
            server_script_time: 0.0,
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
            faction_sentiment_record: None,
        };
        assert_eq!(raw.validate(), Err(ValidationError::UnknownRecordKind(77)));
    }

    // Drift guard: the wire component discriminants MUST stay numeric-equal to the
    // engine `ComponentKind as u16` (crates/entities/src/registry.rs):
    // Transform = 0, PlayerMovement = 5, Mesh = 8. The exhaustive match (no `_`
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
                ComponentPayload::PlayerMovementState(_) => 5,
                ComponentPayload::MeshAnimationState(_) => 8,
                ComponentPayload::KinematicMoverState(_) => 12,
                ComponentPayload::LightState(_) => 1,
                ComponentPayload::EmitterState(_) => 2,
                ComponentPayload::FogState(_) => 4,
                ComponentPayload::TeleporterState(_) => 24,
            };
            assert_eq!(payload.kind(), expected);
        }
        assert_eq!(COMPONENT_KIND_LIGHT, 1);
        assert_eq!(COMPONENT_KIND_BILLBOARD_EMITTER, 2);
        assert_eq!(COMPONENT_KIND_FOG_VOLUME, 4);
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
                projectile_presentation: None,
                presentation_binding: None,
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
                projectile_presentation: None,
                presentation_binding: None,
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
            teleport_yaw_stamp: 0.0,
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
            server_script_time: 0.0,
            version: SNAPSHOT_VERSION,
            sequence: 1,
            server_tick: 1,
            records: vec![record],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
            faction_sentiment_record: None,
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
                projectile_presentation: None,
                presentation_binding: None,
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
                projectile_presentation: None,
                presentation_binding: None,
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
    fn metadata_on_despawn_display_describes_forbidden_metadata() {
        let text = ValidationError::MetadataOnDespawn.to_string();
        assert!(text.contains("forbidden metadata"));
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
            server_script_time: 0.0,
            version: SNAPSHOT_VERSION,
            sequence: 1,
            server_tick: 1,
            records: vec![record],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
            faction_sentiment_record: None,
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
                projectile_presentation: None,
                presentation_binding: None,
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
                projectile_presentation: None,
                presentation_binding: None,
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
                projectile_presentation: None,
                presentation_binding: None,
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
        let mutators: [fn(&mut WirePlayerMovementState); 20] = [
            |m| m.knockback_velocity[0] = f32::NAN,
            |m| m.push_velocity[0] = f32::NAN,
            |m| m.push_velocity[1] = f32::INFINITY,
            |m| m.push_velocity[2] = f32::NEG_INFINITY,
            |m| m.knockback_velocity[1] = f32::INFINITY,
            |m| m.knockback_velocity[2] = f32::NEG_INFINITY,
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
                teleporter: None,
                component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
                transform: None,
                player_movement: Some(movement.clone()),
                mesh_animation_state: None,
                kinematic_mover: None,
                light: None,
                emitter: None,
                fog: None,
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
            teleport: Default::default(),
            movement_state: WireMovementState::Dash {
                elapsed_ms: 10.0,
                boost: [1.0, f32::NAN, 3.0],
            },
            ..sample_movement()
        };
        let payload = RawComponentPayload {
            teleporter: None,
            component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            transform: None,
            player_movement: Some(movement.clone()),
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
        };
        assert_eq!(
            payload.validate(),
            Err(ValidationError::NonFiniteMovementState)
        );
    }

    fn vault_payload(state: WireMovementState) -> RawComponentPayload {
        RawComponentPayload {
            teleporter: None,
            component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            transform: None,
            player_movement: Some(WirePlayerMovementState {
                teleport: Default::default(),
                movement_state: state,
                ..sample_movement()
            }),
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
        }
    }

    fn vault_state(
        elapsed_ms: f32,
        target: [f32; 3],
        start_offset: [f32; 3],
        entry_speed: f32,
    ) -> WireMovementState {
        WireMovementState::Vault {
            elapsed_ms,
            target,
            start_offset,
            mover: Some(77),
            entry_speed,
        }
    }

    // A vault's path and exit read every payload float, and its exit direction
    // normalizes the horizontal start offset: each must be sound before replay.
    #[test]
    fn vault_payload_rejects_non_finite_and_out_of_contract_values_before_typed_apply() {
        let target = [5.4, 1.2, 0.0];
        let offset = [-0.8, -0.7, 0.0];
        assert!(
            vault_payload(vault_state(16.0, target, offset, 7.0))
                .validate()
                .is_ok(),
            "a probe-shaped vault passes"
        );
        for state in [
            vault_state(f32::NAN, target, offset, 7.0),
            vault_state(16.0, [5.4, f32::INFINITY, 0.0], offset, 7.0),
            vault_state(16.0, target, [-0.8, f32::NAN, 0.0], 7.0),
            vault_state(16.0, target, offset, f32::INFINITY),
        ] {
            assert_eq!(
                vault_payload(state).validate(),
                Err(ValidationError::NonFiniteMovementState),
                "{state:?}"
            );
        }
        for state in [
            vault_state(-1.0, target, offset, 7.0),
            vault_state(16.0, target, offset, -0.5),
            vault_state(16.0, target, [0.0005, -0.7, 0.0], 7.0),
            // Finite but huge: would overflow replay's path arithmetic.
            vault_state(16.0, [2.0e38, 1.2, 0.0], offset, 7.0),
            vault_state(16.0, target, [2.0e38, -0.7, 0.0], 7.0),
            vault_state(16.0, target, [60.0, 60.0, 60.0], 7.0),
        ] {
            assert_eq!(
                vault_payload(state).validate(),
                Err(ValidationError::InvalidMovementState),
                "{state:?}"
            );
        }
    }

    // Regression: a finite vertical slide boost previously passed raw validation
    // and could materialize an invalid engine movement state.
    #[test]
    fn vertical_sliding_boost_rejects_before_typed_apply() {
        let movement = WirePlayerMovementState {
            teleport: Default::default(),
            movement_state: WireMovementState::Sliding {
                elapsed_ms: 10.0,
                boost: [1.0, 0.25, 3.0],
                eye_current: 0.5,
                floor_normal: Some([0.0, 1.0, 0.0]),
            },
            ..sample_movement()
        };
        let payload = RawComponentPayload {
            teleporter: None,
            component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            transform: None,
            player_movement: Some(movement.clone()),
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
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
                teleport: Default::default(),
                movement_state: WireMovementState::Sliding {
                    elapsed_ms: 10.0,
                    boost: [1.0, 0.0, 3.0],
                    eye_current: 0.5,
                    floor_normal: Some(floor_normal),
                },
                ..sample_movement()
            };
            let payload = RawComponentPayload {
                teleporter: None,
                component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
                transform: None,
                player_movement: Some(movement.clone()),
                mesh_animation_state: None,
                kinematic_mover: None,
                light: None,
                emitter: None,
                fog: None,
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
                teleport: Default::default(),
                movement_state: WireMovementState::Sliding {
                    elapsed_ms: 10.0,
                    boost: [1.0, 0.0, 3.0],
                    eye_current: 0.5,
                    floor_normal,
                },
                ..sample_movement()
            };
            let payload = RawComponentPayload {
                teleporter: None,
                component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
                transform: None,
                player_movement: Some(movement.clone()),
                mesh_animation_state: None,
                kinematic_mover: None,
                light: None,
                emitter: None,
                fog: None,
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
                teleporter: None,
                component_kind: COMPONENT_KIND_KINEMATIC_MOVER_STATE,
                transform: None,
                player_movement: None,
                mesh_animation_state: None,
                kinematic_mover: Some(mover),
                light: None,
                emitter: None,
                fog: None,
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
                teleporter: None,
                component_kind: COMPONENT_KIND_KINEMATIC_MOVER_STATE,
                transform: None,
                player_movement: None,
                mesh_animation_state: None,
                kinematic_mover: Some(mover),
                light: None,
                emitter: None,
                fog: None,
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
                teleporter: None,
                component_kind: COMPONENT_KIND_TRANSFORM,
                transform: Some(transform),
                player_movement: None,
                mesh_animation_state: None,
                kinematic_mover: None,
                light: None,
                emitter: None,
                fog: None,
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
                teleporter: None,
                component_kind: COMPONENT_KIND_TRANSFORM,
                transform: Some(bad_transform),
                player_movement: None,
                mesh_animation_state: None,
                kinematic_mover: None,
                light: None,
                emitter: None,
                fog: None,
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
                teleporter: None,
                component_kind: COMPONENT_KIND_TRANSFORM,
                transform: Some(bad_transform),
                player_movement: None,
                mesh_animation_state: None,
                kinematic_mover: None,
                light: None,
                emitter: None,
                fog: None,
            }],
        );
        record.has_entity_class = true;
        record.entity_class = "player".to_string();
        assert_eq!(record.validate(), Err(ValidationError::NonFiniteTransform));
    }

    fn projectile_facts() -> WireProjectilePresentation {
        WireProjectilePresentation {
            sprite_size: Some(0.4),
            model_scale: 2.0,
            shot_id: Some(WireShotId {
                pawn: 41,
                start_tick: 12,
                lane: 1,
                ordinal: 2,
            }),
        }
    }

    #[test]
    fn projectile_presentation_rejects_malformed_scalars_identity_and_record_shape() {
        let mut record = raw_record(
            RECORD_KIND_FULL_BASELINE,
            4,
            1,
            0,
            0,
            vec![raw_transform_payload()],
        );
        record.has_entity_class = true;
        record.entity_class = "projectile:plasma".into();
        record.projectile_presentation = Some(projectile_facts());
        assert!(record.validate().is_ok());
        for value in [f32::NAN, f32::INFINITY, 0.0, -1.0] {
            record.projectile_presentation = Some(WireProjectilePresentation {
                sprite_size: Some(value),
                ..projectile_facts()
            });
            assert_eq!(
                record.validate(),
                Err(ValidationError::InvalidProjectilePresentation)
            );
            record.projectile_presentation = Some(WireProjectilePresentation {
                model_scale: value,
                ..projectile_facts()
            });
            assert_eq!(
                record.validate(),
                Err(ValidationError::InvalidProjectilePresentation)
            );
        }
        for id in [
            WireShotId {
                lane: 2,
                ..projectile_facts().shot_id.unwrap()
            },
            WireShotId {
                ordinal: 16,
                ..projectile_facts().shot_id.unwrap()
            },
        ] {
            record.projectile_presentation = Some(WireProjectilePresentation {
                shot_id: Some(id),
                ..projectile_facts()
            });
            assert_eq!(
                record.validate(),
                Err(ValidationError::InvalidProjectilePresentation)
            );
        }
        record.projectile_presentation = Some(projectile_facts());
        record.components.clear();
        assert!(record.validate().is_err());
        record.record_kind = RECORD_KIND_DESPAWN;
        assert_eq!(record.validate(), Err(ValidationError::MetadataOnDespawn));
    }

    fn weapon_cue() -> WeaponCue {
        WeaponCue {
            shot_id: projectile_facts().shot_id.unwrap(),
            kind: WeaponCueKind::Impact,
            sound: Some("old_impact".into()),
            additional_sound: None,
            alias: Some("alt_impact".into()),
            anchor: WeaponCueAnchor::Contacts(vec![WeaponCueContact {
                point: [1.0, 2.0, 3.0],
                normal: [0.0, 1.0, 0.0],
                target: Some(NetworkId(7)),
            }]),
        }
    }

    #[test]
    fn weapon_cue_wire_rejects_invalid_identity_names_contact_facts_and_bounds() {
        let message = ServerMessage::WeaponCues(WeaponCuesMessage {
            participation_epoch: 7,
            cues: vec![weapon_cue()],
        });
        assert!(round_trips(&message));
        for lane in [2, 255] {
            let mut cue = weapon_cue();
            cue.shot_id.lane = lane;
            assert!(!cue.is_valid());
        }
        let mut cue = weapon_cue();
        cue.shot_id.ordinal = 16;
        assert!(!cue.is_valid());
        for value in [
            "".to_string(),
            "  ".to_string(),
            "x".repeat(MAX_WEAPON_CUE_NAME_BYTES + 1),
        ] {
            let mut cue = weapon_cue();
            cue.sound = Some(value.clone());
            assert!(!cue.is_valid());
            let mut cue = weapon_cue();
            cue.alias = Some(value);
            assert!(!cue.is_valid());
        }
        let mut cue = weapon_cue();
        cue.alias = Some("impact".into());
        assert!(!cue.is_valid());
        let mut cue = weapon_cue();
        cue.anchor = WeaponCueAnchor::Entity {
            entity: NetworkId(1),
            origin: [f32::NAN, 0.0, 0.0],
        };
        assert!(!cue.is_valid());
        let mut cue = weapon_cue();
        cue.anchor = WeaponCueAnchor::Contacts(Vec::new());
        assert!(!cue.is_valid());
        let mut cue = weapon_cue();
        cue.anchor = WeaponCueAnchor::Contacts(vec![WeaponCueContact {
            point: [0.0; 3],
            normal: [f32::INFINITY; 3],
            target: None,
        }]);
        assert!(!cue.is_valid());
        let mut cue = weapon_cue();
        cue.anchor = WeaponCueAnchor::Contacts(vec![
            WeaponCueContact {
                point: [0.0; 3],
                normal: [0.0; 3],
                target: None
            };
            MAX_WEAPON_CUE_CONTACTS + 1
        ]);
        assert!(!cue.is_valid());
        assert!(
            !WeaponCuesMessage {
                participation_epoch: 7,
                cues: vec![weapon_cue(); MAX_WEAPON_CUES + 1]
            }
            .is_valid()
        );
    }

    #[test]
    fn projectile_presentation_rejects_previous_snapshot_epoch() {
        assert_eq!(raw_snapshot(1, 2, Vec::new()).version, SNAPSHOT_VERSION);
        let mut raw = raw_snapshot(1, 2, Vec::new());
        raw.version = 16;
        assert_eq!(
            raw.validate(),
            Err(ValidationError::VersionMismatch {
                expected: SNAPSHOT_VERSION,
                received: 16
            })
        );
    }

    #[test]
    fn presentation_binding_and_script_clock_roundtrip_and_reject_invalid_shapes() {
        let state = ComponentPayload::EmitterState(WireEmitterState {
            rate: 3.0,
            spin_rate: 2.0,
            spin_animation: None,
            anim_start: Some(4.25),
        });
        let mut record = raw_record(
            RECORD_KIND_FULL_BASELINE,
            7,
            2,
            0,
            0,
            vec![raw_from_typed(&state)],
        );
        record.presentation_binding = Some(WirePresentationBinding {
            kind: 2,
            index: 18,
            origin: [1.0, 2.0, 3.0],
        });
        let mut snapshot = raw_snapshot(1, 2, vec![record.clone()]);
        snapshot.server_script_time = 4.5;
        let decoded: RawSnapshotMessage = decode(&encode(&snapshot)).unwrap();
        assert_eq!(decoded, snapshot);
        let typed = decoded.validate().unwrap();
        assert_eq!(typed.server_script_time, 4.5);
        assert!(
            matches!(&typed.records[0], EntityRecord::FullBaseline { presentation_binding: Some(binding), .. } if binding.index == 18)
        );

        for binding in [
            WirePresentationBinding {
                kind: 3,
                index: 0,
                origin: [0.0; 3],
            },
            WirePresentationBinding {
                kind: 0,
                index: 0,
                origin: [f32::NAN, 0.0, 0.0],
            },
        ] {
            let mut bad = record.clone();
            bad.presentation_binding = Some(binding);
            assert_eq!(
                bad.validate(),
                Err(ValidationError::InvalidPresentationBinding)
            );
        }
        let mut bad = record.clone();
        bad.components.push(raw_transform_payload());
        assert_eq!(
            bad.validate(),
            Err(ValidationError::InvalidPresentationBinding)
        );
        let mut bad = record.clone();
        bad.has_entity_class = true;
        bad.entity_class = "prop".into();
        assert!(bad.validate().is_err());
        let mut bad = record.clone();
        bad.components.clear();
        assert_eq!(
            bad.validate(),
            Err(ValidationError::InvalidPresentationBinding)
        );
        let mut bad = record.clone();
        bad.presentation_binding = None;
        assert_eq!(
            bad.validate(),
            Err(ValidationError::PresentationStateWithoutTransform)
        );
        bad.components.push(raw_transform_payload());
        assert!(bad.validate().is_ok());
        let mut bad = record;
        bad.record_kind = RECORD_KIND_DESPAWN;
        assert_eq!(bad.validate(), Err(ValidationError::MetadataOnDespawn));
        snapshot.server_script_time = f64::INFINITY;
        assert_eq!(
            snapshot.validate(),
            Err(ValidationError::NonFiniteScriptTime)
        );
        snapshot.server_script_time = 0.0;
        // The layout before the vault movement state.
        snapshot.version = 19;
        assert!(matches!(
            snapshot.validate(),
            Err(ValidationError::VersionMismatch {
                expected: 21,
                received: 19
            })
        ));
    }

    fn raw_from_typed(payload: &ComponentPayload) -> RawComponentPayload {
        match payload {
            ComponentPayload::TeleporterState(state) => RawComponentPayload {
                component_kind: crate::wire::COMPONENT_KIND_TELEPORTER,
                teleporter: Some(*state),
                transform: None,
                player_movement: None,
                mesh_animation_state: None,
                kinematic_mover: None,
                light: None,
                emitter: None,
                fog: None,
            },
            ComponentPayload::Transform(t) => RawComponentPayload {
                teleporter: None,
                component_kind: COMPONENT_KIND_TRANSFORM,
                transform: Some(*t),
                player_movement: None,
                mesh_animation_state: None,
                kinematic_mover: None,
                light: None,
                emitter: None,
                fog: None,
            },
            ComponentPayload::PlayerMovementState(m) => RawComponentPayload {
                teleporter: None,
                component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
                transform: None,
                player_movement: Some(m.clone()),
                mesh_animation_state: None,
                kinematic_mover: None,
                light: None,
                emitter: None,
                fog: None,
            },
            ComponentPayload::MeshAnimationState(m) => RawComponentPayload {
                teleporter: None,
                component_kind: COMPONENT_KIND_MESH_ANIMATION_STATE,
                transform: None,
                player_movement: None,
                mesh_animation_state: Some(m.clone()),
                kinematic_mover: None,
                light: None,
                emitter: None,
                fog: None,
            },
            ComponentPayload::KinematicMoverState(m) => RawComponentPayload {
                teleporter: None,
                component_kind: COMPONENT_KIND_KINEMATIC_MOVER_STATE,
                transform: None,
                player_movement: None,
                mesh_animation_state: None,
                kinematic_mover: Some(*m),
                light: None,
                emitter: None,
                fog: None,
            },
            ComponentPayload::LightState(state) => RawComponentPayload {
                teleporter: None,
                component_kind: COMPONENT_KIND_LIGHT,
                transform: None,
                player_movement: None,
                mesh_animation_state: None,
                kinematic_mover: None,
                light: Some(state.clone()),
                emitter: None,
                fog: None,
            },
            ComponentPayload::EmitterState(state) => RawComponentPayload {
                teleporter: None,
                component_kind: COMPONENT_KIND_BILLBOARD_EMITTER,
                transform: None,
                player_movement: None,
                mesh_animation_state: None,
                kinematic_mover: None,
                light: None,
                emitter: Some(state.clone()),
                fog: None,
            },
            ComponentPayload::FogState(state) => RawComponentPayload {
                teleporter: None,
                component_kind: COMPONENT_KIND_FOG_VOLUME,
                transform: None,
                player_movement: None,
                mesh_animation_state: None,
                kinematic_mover: None,
                light: None,
                emitter: None,
                fog: Some(state.clone()),
            },
        }
    }
    // --- Teleporters ---

    #[test]
    fn teleport_record_snapshot_rejects_the_pre_teleport_layout() {
        const PRE_TELEPORT_SNAPSHOT_VERSION: u16 = 20;
        assert_eq!(SNAPSHOT_VERSION, PRE_TELEPORT_SNAPSHOT_VERSION + 1);
        let mut raw = raw_snapshot(1, 2, Vec::new());
        raw.version = PRE_TELEPORT_SNAPSHOT_VERSION;
        assert_eq!(
            raw.validate(),
            Err(ValidationError::VersionMismatch {
                expected: SNAPSHOT_VERSION,
                received: PRE_TELEPORT_SNAPSHOT_VERSION,
            })
        );
    }

    #[test]
    fn teleporter_state_rejects_the_sentinel_index_as_a_value_error() {
        let raw = |index| RawComponentPayload {
            teleporter: Some(WireTeleporterState {
                index,
                source_entity: 3,
                armed: true,
            }),
            component_kind: COMPONENT_KIND_TELEPORTER,
            transform: None,
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
        };
        assert!(matches!(
            raw(0).validate(),
            Ok(ComponentPayload::TeleporterState(state)) if state.index == 0
        ));
        assert_eq!(
            raw(NO_WIRE_TELEPORTER).validate(),
            Err(ValidationError::InvalidTeleporterState)
        );
        let mut doubled = raw(0);
        doubled.transform = Some(sample_transform());
        assert_eq!(
            doubled.validate(),
            Err(ValidationError::MismatchedComponentPayload(
                COMPONENT_KIND_TELEPORTER
            ))
        );
    }

    #[test]
    fn teleport_record_movement_payload_rejects_an_invalid_latch_set() {
        let mut payload = raw_movement_payload();
        payload.player_movement.as_mut().unwrap().teleport =
            WireTeleportState::new(4, 1.0, 1, &[0, 2]).unwrap();
        assert!(payload.validate().is_ok());
        let mut bad = payload.clone();
        bad.player_movement
            .as_mut()
            .unwrap()
            .teleport
            .accumulated_yaw = f32::NAN;
        assert!(bad.validate().is_err());
    }
}

#[cfg(test)]
mod activation_tests {
    use super::*;
    #[test]
    fn activation_wire_round_trips_full_identity_and_terminal_outcomes() {
        let token = WireActivationToken {
            start_tick: u32::MAX,
            lane: 1,
        };
        let id = WireShotId {
            pawn: 41,
            start_tick: token.start_tick,
            lane: token.lane,
            ordinal: 15,
        };
        let declaration = HitDeclaration {
            shot_id: id,
            records: Vec::new(),
        };
        assert_eq!(
            decode::<HitDeclaration>(&encode(&declaration)).unwrap(),
            declaration
        );
        let message = ServerMessage::ActivationOutcomes(vec![
            ActivationOutcome::InitiationAccepted {
                token,
                weapon: NetworkId(9),
            },
            ActivationOutcome::ExecutionAccepted {
                token,
                weapon: NetworkId(9),
                charge_millionths: 1_000_000,
                recovery_ticks: 18,
            },
            ActivationOutcome::Cancelled {
                token,
                weapon: NetworkId(9),
                recovery_ticks: 18,
            },
            ActivationOutcome::Completed {
                token,
                weapon: NetworkId(9),
                recovery_ticks: 18,
            },
            ActivationOutcome::InitiationRejected {
                token,
                recovery_ticks: 18,
            },
        ]);
        assert_eq!(decode::<ServerMessage>(&encode(&message)).unwrap(), message);
        let other_lane = WireShotId { lane: 0, ..id };
        let other_ordinal = WireShotId { ordinal: 14, ..id };
        assert_ne!(id, other_lane);
        assert_ne!(id, other_ordinal);
    }
}
