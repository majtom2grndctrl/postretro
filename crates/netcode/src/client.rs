// Client-side snapshot apply/repair/ack state, plus local reconcile and
// descriptor-remote materialization surfaces.
// See: context/lib/networking.md
//
// This is the engine half of the client replication data path. The net crate is
// registry-blind and keyed only by `NetworkId`; this module owns the engine side
// that must know both halves: it decides how each validated `EntityRecord` mutates
// the `EntityRegistry`, tracks which `baseline_id` it holds per entity, and decides
// when an unappliable record needs a full-baseline refresh. It also surfaces
// authoritative local-pawn inputs for reconciliation and descriptor-class remote
// requests for presentation materialization, including retries while descriptor
// presentation is incomplete. All registry mutation flows through the
// game-logic-owned apply primitives (`spawn`, `set_component_value`, `despawn`) —
// the net crate never touches the registry.
//
// State machine (per validated snapshot, applied in record order):
//   - Bound FullBaseline: resolve the authored local entity; do not spawn or apply
//     Transform. Map the identity, collect presentation state, and ack. A rejected
//     binding is remembered and its record is acked without repair; later records for
//     that rejected identity are also acked and skipped.
//   - Ordinary FullBaseline, unmapped: spawn (Transform required), apply payloads,
//     record the map + baseline, clear pending repair, ack.
//   - FullBaseline, mapped + live: update in place, clear pending repair, ack.
//   - FullBaseline, mapped + stale entity: drop the stale mapping, request refresh;
//     do not ack the entity baseline.
//   - Delta, some baseline held, new_baseline_id newer: apply regardless of
//     baseline_ref (a delta carries the full component set), advance the stored
//     baseline, ack. Newest wins: the host keeps naming its last acked ref while
//     the client's acks are in flight.
//   - Delta, new_baseline_id equal to the held one: a metadata-only resend (the
//     host refreshes authority metadata without bumping an unchanged baseline).
//     Re-run the metadata consumers (reconcile capture, arming, materialize
//     retry); no component re-apply, no refresh, no ack. A presentation-bound
//     entity has no metadata consumers and skips it.
//   - Delta, new_baseline_id older than the held one: inside an accepted newest
//     snapshot this breaks newest-wins (a namespace regression, or a gap past
//     half the id range), so request refresh, leave state untouched, and do not
//     ack.
//   - Delta, no held baseline or no live mapping: request refresh, leave state
//     untouched, and do not ack the entity baseline.
//   - Despawn: retire the network identity and ack its tombstone. Despawn a
//     replication-created copy; only unbind an authored presentation entity.
//   - Older snapshot: only a first baseline for an unseen identity may apply.
//     Duplicate snapshots and other older records are ignored; clocks stay newest.

use glam::Vec3;
use std::collections::{HashMap, HashSet, VecDeque};

use postretro_net::wire::{
    AckMessage, BaselineRefreshRequest, COMPONENT_KIND_PLAYER_MOVEMENT_STATE, ClientMessage,
    ComponentPayload, EntityRecord, NetworkId, SnapshotMessage, WireGroundRef,
    WireKinematicMoverState, WirePlayerMovementState,
};

use postretro_entities::components::mesh::{
    LocomotionDirection, MeshAnimation, MeshComponent, SwitchResult, switch_animation_state,
};
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent,
    KinematicMoverMode, Transform,
};

use super::interpolation::{
    ObservedTeleport, RemoteInterpolationBuffer, TeleportSample, TransformSample,
};
use super::{payload_is_finite, wire_to_transform};
use crate::collision::moving::{MoverPose, MoverPoseSource};
use crate::kinematic_mover::{advance_mover_phase_one_tick, mover_pose_for_current_phase};

mod bindings;
#[cfg(test)]
mod ghost_link_tests;
mod ghost_links;
#[cfg(test)]
mod late_baseline_tests;
mod late_baselines;
#[cfg(test)]
mod presentation_tests;
mod replay_poses;
#[cfg(test)]
mod teleporter_tests;
mod teleporters;
use bindings::{ClientBindings, RegionBinding, is_presentation_payload};
pub use ghost_links::GhostLinkUpdate;
use late_baselines::LateBaselineHistory;
use postretro_entities::presentation_binding::PresentationBindingResolver;
use postretro_entities::presentation_binding::{PresentationBindingKey, PresentationBindingKind};

#[cfg(test)]
pub(crate) struct NoPresentationBindings;
#[cfg(test)]
impl PresentationBindingResolver for NoPresentationBindings {
    fn resolve(&self, _: &EntityRegistry, _: PresentationBindingKey) -> Option<(EntityId, Vec3)> {
        None
    }
    fn key_for_entity(
        &self,
        _: &EntityRegistry,
        _: EntityId,
    ) -> Option<(PresentationBindingKey, Vec3)> {
        None
    }
}

const MOVER_HISTORY_LIMIT: usize = 128;
#[cfg(test)]
const DEFAULT_MOVER_TICK_DT: f32 = 1.0 / 60.0;

/// Result of one remote interpolation sampling pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct InterpolationSampleStats {
    pub(crate) presented: usize,
    pub(crate) held_newest: usize,
    pub(crate) starvation_feedback: usize,
}

/// Reason code carried in a `BaselineRefreshRequest`. Diagnostic only — the server
/// repair path keys on entity + missing ref, not the reason (net wire contract).
/// A `Delta` arrived with no held baseline, or a regressed one.
const REFRESH_REASON_UNKNOWN_BASELINE: u8 = 0;
/// Reason: a `FullBaseline` named a `NetworkId` whose mapped `EntityId` was stale.
const REFRESH_REASON_STALE_MAPPING: u8 = 1;
/// Reason: local level unload cleared registry-backed entities but kept the transport.
const REFRESH_REASON_LEVEL_RELOAD: u8 = 2;
/// Reason: the armed local pawn's first record has not merged (its host tuning
/// installed after the record arrived), so the client asks for it again.
pub(crate) const REFRESH_REASON_UNMERGED_LOCAL_PAWN: u8 = 3;

/// Repair-request resend cadence: one `BaselineRefreshRequest` per pending entity
/// every 200 ms (5 Hz) until the matching full baseline arrives and clears it. The
/// reliable `Channel::Input` makes a single request sufficient in the common case;
/// the cadence covers the entity falling out of and back into the pending set.
const REPAIR_RESEND_INTERVAL_MS: f32 = 200.0;

/// Whether `candidate` was issued after `held` by a host baseline allocator. Ids
/// are `u32` from `wrapping_add` allocators that never restart for an endpoint's
/// lifetime, so this is a serial-number comparison. Full-state deltas apply on
/// this rule; a `FullBaseline` replaces the held id unconditionally. The host's
/// ack paths compare with plain `>`, which disagrees only past a `u32` wrap.
pub(crate) fn baseline_is_newer(candidate: u32, held: u32) -> bool {
    (candidate.wrapping_sub(held) as i32) > 0
}

/// How [`ClientReplication::apply_delta`] disposed of one `Delta`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeltaApply {
    /// Components applied and the stored baseline advanced; ack it.
    Applied,
    /// Same baseline id as held: a metadata-only resend. Metadata consumers run;
    /// nothing re-applies or acks.
    MetadataRefresh,
    /// Nothing applied: a refresh was queued, or a payload failed to apply.
    Rejected,
}

/// Descriptor-immutable locomotion reference data for a remote enemy. The host
/// replicates only its changing pose and mesh state; the client reads this from the
/// shared descriptor table when that remote is materialized.
#[derive(Debug, Clone)]
struct RemoteEnemyWalkPlayback {
    move_speed: f32,
    walk_state: String,
    /// Load-derived speed for the alert state's clip. The runtime state applies
    /// its authored `travelSpeed` override before this value.
    derived_travel_speed: Option<f32>,
}

/// Descriptor-derived locomotion data for a remote player avatar. Remote player
/// meshes deliberately remain presentation-only; this cache lets the client derive
/// idle/walk/run/crouch/jump and forward/backward from replicated intrinsic
/// velocity, posture and footing without attaching PlayerMovement.
#[derive(Debug, Clone)]
struct RemotePlayerLocomotion {
    reference: RemotePlayerLocomotionReference,
    /// Last state selected from the interpolated intrinsic player velocity. A due
    /// host state is compared against this prediction before correcting.
    client_derived_state: Option<String>,
    /// Server state currently honored over this client's prediction. The velocity
    /// derivation does not switch away from it until the derivation agrees with it
    /// or the host names another state.
    authoritative_correction_state: Option<String>,
    /// Host states received but not yet presented, oldest first, each with the
    /// server tick it was selected on. A state applies once the presented pose
    /// reaches its tick, so it never leads the body it describes.
    pending_corrections: VecDeque<PendingAnimationCorrection>,
    /// The jump phase has no presented history to advance from: the avatar was
    /// just cached, or a held newest pose is about to hand over to an older
    /// interpolated one. The next interpolated presentation reads the phase from
    /// its footing instead.
    resync_jump_phase: bool,
}

/// Which pose a remote avatar's locomotion is selected from this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemotePlayerPresentation {
    /// The interpolation sample at the render tick.
    Interpolated,
    /// The newest applied pose, held before time sync names a render tick.
    HeldNewest,
}

/// One host animation state awaiting presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingAnimationCorrection {
    server_tick: u32,
    state: String,
}

/// Host states queued per remote avatar. Snapshots outpace presentation by the
/// interpolation delay (half the round trip plus 50–250 ms of buffer),
/// and a resend of the newest queued state is not queued again, so the bound is
/// reached only when the delay overruns; the oldest state, superseded by the
/// rest, is dropped first.
const MAX_PENDING_ANIMATION_CORRECTIONS: usize = 16;

impl RemotePlayerLocomotion {
    fn new(reference: RemotePlayerLocomotionReference) -> Self {
        Self {
            reference,
            client_derived_state: None,
            authoritative_correction_state: None,
            pending_corrections: VecDeque::new(),
            resync_jump_phase: true,
        }
    }

    fn queue_correction(&mut self, server_tick: u32, state: &str) {
        let queue = &mut self.pending_corrections;
        // Accepted snapshot ticks only advance; one at or before the newest
        // queued tick supersedes it.
        while queue
            .back()
            .is_some_and(|pending| pending.server_tick >= server_tick)
        {
            queue.pop_back();
        }
        // The host resends an unchanged state with every delta; the earliest
        // tick naming it is when it began.
        if queue.back().is_some_and(|pending| pending.state == state) {
            return;
        }
        if queue.len() == MAX_PENDING_ANIMATION_CORRECTIONS {
            queue.pop_front();
        }
        queue.push_back(PendingAnimationCorrection {
            server_tick,
            state: state.to_string(),
        });
    }

    /// The newest queued state whose tick the presentation has reached, dropping
    /// it and every older one it supersedes. `None` for `presented_tick` releases
    /// the whole queue.
    fn take_due_correction(&mut self, presented_tick: Option<f64>) -> Option<String> {
        let mut due = None;
        while let Some(pending) = self.pending_corrections.front() {
            if presented_tick.is_some_and(|tick| f64::from(pending.server_tick) > tick) {
                break;
            }
            due = self
                .pending_corrections
                .pop_front()
                .map(|pending| pending.state);
        }
        due
    }
}

/// Immutable descriptor values needed for a remote player avatar's client-local
/// locomotion. The descriptor-aware receive glue builds this while model metadata is
/// available; the replication state owns the resulting per-network-id cache.
#[derive(Debug, Clone)]
pub(crate) struct RemotePlayerLocomotionReference {
    pub(crate) idle_state: String,
    pub(crate) states: crate::sim::PlayerLocomotionStates,
    pub(crate) walk_speed: f32,
    pub(crate) run_speed: f32,
    pub(crate) crouch_speed: f32,
    /// Load-derived clip travel speed per locomotion state. States whose clip
    /// derives none are absent.
    pub(crate) derived_travel_speeds: HashMap<String, f32>,
    /// Clip length per jump one-shot state, which ends it. States whose clip
    /// the model lacks are absent.
    pub(crate) clip_durations: HashMap<String, f32>,
}

#[cfg(test)]
impl RemotePlayerLocomotionReference {
    /// An `idle` + `walk_forward` avatar with no derived clip speeds.
    pub(crate) fn idle_and_walk_forward(speed: f32) -> Self {
        Self::declaring(&["idle", "walk_forward"], speed, speed)
    }

    pub(crate) fn declaring(states: &[&str], walk_speed: f32, run_speed: f32) -> Self {
        Self {
            idle_state: "idle".to_string(),
            states: crate::sim::PlayerLocomotionStates::from_declared(|state| {
                states.contains(&state)
            })
            .expect("a walk state is declared"),
            walk_speed,
            run_speed,
            crouch_speed: 3.0,
            derived_travel_speeds: HashMap::new(),
            clip_durations: HashMap::new(),
        }
    }
}

/// Per-frame interpolation results consumed by the presentation pose-input pass.
/// The maps retain `NetworkId` identity until the caller joins them to the client
/// mapping, keeping remote avatar state outside the entity component vocabulary.
#[derive(Debug, Clone, Default)]
pub struct ClientPresentationInputs {
    pub aim_pitches: HashMap<NetworkId, f32>,
    pub heading_yaws: HashMap<NetworkId, f32>,
    /// The server tick every remote entity was sampled at this frame, exactly as the
    /// interpolation used it. `None` before time sync names a render tick, while
    /// remotes hold their newest applied pose.
    pub render_server_tick: Option<f64>,
}

const MIN_PRESENTED_HORIZONTAL_SPEED_SQ: f32 = 1.0e-8;

fn horizontal_speed(velocity: Vec3) -> f32 {
    Vec3::new(velocity.x, 0.0, velocity.z).length()
}

fn is_horizontal_motion(velocity: &Vec3) -> bool {
    velocity.x * velocity.x + velocity.z * velocity.z > MIN_PRESENTED_HORIZONTAL_SPEED_SQ
}

/// A wire payload the client received but deliberately did not apply, recorded as a
/// typed diagnostic rather than silently dropped. A remote pawn's
/// `PlayerMovementState` has no local descriptor-derived `PlayerMovementComponent`
/// to merge onto, so it is not merged onto a component; its velocity, stance, aim
/// pitch and teleport sample still ride the interpolation sample to presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IgnoredPayload {
    /// A `PlayerMovementState` payload arrived for an entity with no local
    /// `PlayerMovementComponent` to merge it onto. The `Transform` (if present) was
    /// still applied, and the movement subset still fed presentation; only the
    /// component merge was skipped.
    MovementWithoutLocalComponent { network_id: u32 },
    /// A mover payload referenced a PRL mover id this client did not load. This is
    /// an engine-side validation failure because the net crate is registry-blind.
    UnknownKinematicMover { network_id: u32, mover_id: u32 },
    /// A movement payload referenced a ground mover id this client did not load.
    /// The ground reference is engine-owned validation for the same reason.
    UnknownGroundMover { network_id: u32, mover_id: u32 },
    /// A teleporter baseline named an index and source entity no installed
    /// teleporter carries. Nothing binds; the region keeps its local armed state.
    UnknownTeleporter {
        network_id: u32,
        index: u16,
        source_entity: u32,
    },
    /// A ghost-link baseline named an index and source entity no installed link
    /// carries. Nothing binds or queues; the link keeps its local spawn state.
    UnknownGhostLink {
        network_id: u32,
        index: u16,
        source_entity: u32,
    },
}

/// One pending baseline-repair entry: the entity needs a full baseline re-sent and
/// the client resends a `BaselineRefreshRequest` on the 5 Hz cadence until it
/// arrives. `missing_baseline_ref` and `reason` ride the request for diagnostics;
/// `since_last_request_ms` accumulates frame dt to drive the cadence.
#[derive(Debug, Clone, Copy)]
struct PendingRepair {
    missing_baseline_ref: u32,
    snapshot_sequence: u32,
    reason: u8,
    since_last_request_ms: f32,
}

/// Client replication state: the `NetworkId -> EntityId` map, the stored baseline id
/// per mapped entity, the pending-repair set, and the latest accepted snapshot
/// sequence + acked server tick. The single owner of client-side replication state
/// and the only client code that mutates the registry on replication's behalf.
#[derive(Debug, Default)]
pub struct ClientReplication {
    /// `NetworkId -> EntityId` for every live replication mapping, including authored
    /// presentation entities resolved from a binding as well as spawned copies.
    map: HashMap<NetworkId, EntityId>,
    /// Reverse of `map`, maintained in lockstep so client-local hit records can name
    /// their target on the wire.
    reverse_map: HashMap<EntityId, NetworkId>,
    /// `NetworkId -> stored baseline_id`. The baseline the client currently holds for
    /// each mapped entity. A `Delta` applies when its `new_baseline_id` is newer than
    /// this (see [`baseline_is_newer`]), whatever its `baseline_ref`, and a successful
    /// apply advances it. Kept in lockstep with `map`.
    baselines: HashMap<NetworkId, u32>,
    /// Last active-weapon archetype applied for each replicated movement pawn. This
    /// is presentation state only: it detects shared-visible equip changes without
    /// deriving a weapon from owner-private data or mutating gameplay components.
    active_weapon_archetypes: HashMap<NetworkId, Option<String>>,
    /// Replicated class retained for each remote presentation entity, as the
    /// snapshot tagged it (`descriptor:` / `pawn:` / `projectile:`). Overlay
    /// anchoring uses the local descriptor's authored hitbox when a model socket
    /// or derived skeletal bound is unavailable; no combat component is created.
    remote_entity_classes: HashMap<NetworkId, String>,
    projectile_presentations: HashMap<NetworkId, postretro_net::wire::WireProjectilePresentation>,
    /// Entities awaiting a full-baseline refresh, keyed by `NetworkId`. An entry here
    /// resends a `BaselineRefreshRequest` on the 5 Hz cadence; the matching
    /// `FullBaseline` apply clears it.
    pending_repairs: HashMap<NetworkId, PendingRepair>,
    /// The local pawn whose unmerged-first-record refresh has already sent its one
    /// immediate request; later requests for it wait on the resend cadence.
    unmerged_refresh_sent: Option<NetworkId>,
    /// Highest accepted snapshot sequence. Late first entity baselines may apply
    /// without advancing this cursor or any snapshot-wide state.
    latest_sequence: Option<u32>,
    late_baselines: LateBaselineHistory,
    /// The `server_tick` of the latest accepted snapshot — echoed back in the ack.
    acked_server_tick: u32,
    /// Per-remote-entity interpolation buffers keyed by `NetworkId`. Each
    /// applied `Transform` payload is recorded here stamped by the snapshot's
    /// `server_tick`; `sample_into_registry` later resolves a presented pose for the
    /// render target tick and writes it through the registry's remote-presentation
    /// helper. The raw `set_component_value` in `apply_components_to` only seeds the
    /// entity's initial pose at spawn — the interpolation sampler drives the visible
    /// pose every frame thereafter.
    interp: RemoteInterpolationBuffer,
    /// Descriptor-derived walk-rate references for remote AI enemies. This stays
    /// beside the interpolation buffers because it is consumed by the same
    /// per-frame client presentation step, but never crosses the wire.
    remote_enemy_walk_playback: HashMap<NetworkId, RemoteEnemyWalkPlayback>,
    /// Descriptor-aware materialization reports completion independently of Mesh.
    /// Kept only for live mappings; unknown/incomplete classes remain retryable.
    remote_presentation_complete: HashSet<NetworkId>,
    /// Descriptor-derived player locomotion references keyed by remote identity.
    /// Presence is also the durable signal that a transform-only remote mesh is a
    /// player avatar rather than an AI presentation.
    remote_player_locomotion: HashMap<NetworkId, RemotePlayerLocomotion>,
    /// Current-frame aim and lower-body heading extracted from the exact presented
    /// interpolation poses. Cleared before each sample pass and copied to App after
    /// interpolation so the later pose-input pass shares this frame's trajectory.
    presented_player_inputs: ClientPresentationInputs,
    /// The local predicted pawn's `NetworkId`, once a `local_player` record has armed
    /// it (M15 Phase 3 Task 5). The local pawn is driven by client-side prediction +
    /// reconciliation, NOT the remote interpolation path: it is excluded from the
    /// interp buffer (`apply_components_to` skips recording it) and from
    /// `sample_into_registry`'s presentation writes (which would otherwise clobber the
    /// reconciled pose with a stale interpolated remote pose). `None` until armed.
    local_pawn: Option<NetworkId>,
    /// Descriptor class carried by the latest local-player record. Control tuning
    /// can arrive after that record, so the independent Control drain retains this
    /// half of the materialization key for an immediate movement rebuild.
    local_pawn_entity_class: Option<String>,
    /// Local identity bindings used by snapshot apply.
    bindings: ClientBindings,
    /// Ghost-link states received since presentation last drained them
    /// ([`Self::drain_ghost_link_updates`]). Dropped with every other received
    /// state at unload and demotion.
    ghost_link_updates: ghost_links::PendingGhostLinkUpdates,
    /// The newest snapshot's level-scoped ghost-audio record, until presentation
    /// takes it ([`Self::take_ghost_audio`]). Dropped with every other received
    /// state at unload and demotion.
    ghost_audio: Option<postretro_net::wire::WireGhostAudioState>,
    /// Per remote, the host tick whose state this frame's presented pose shows
    /// ([`Self::presented_state_tick`]). Rebuilt each sample pass.
    presented_state_ticks: HashMap<NetworkId, f64>,
    /// Authoritative mover samples from host snapshots, keyed by stable PRL
    /// `mover_id`. This is distinct from the live per-tick side-table. Reconcile
    /// replay reads per-tick poses from it through `resolve_replay_tick` and
    /// `replay_pose`; its `MoverPoseSource` impl exposes only the newest sample.
    mover_history: MoverHistoryBuffer,
    /// Off-by-default remote-interpolation continuity diagnostics (see `netdiag`).
    /// Inert unless `postretro::netdiag=debug`; pure observability, never gameplay.
    netdiag: crate::netcode::netdiag::RemoteInterpDiag,
}

#[derive(Debug, Clone)]
pub(crate) struct MoverHistorySample {
    pub(crate) server_tick: u32,
    pub(crate) pose: MoverPose,
    pub(crate) phase: KinematicMoverComponent,
}

#[derive(Debug, Default)]
pub(crate) struct MoverHistoryBuffer {
    samples: HashMap<u32, VecDeque<MoverHistorySample>>,
    /// Bumped by every change to `samples`; keys the replay memo's working phases.
    generation: u64,
    replay: replay_poses::ReplayPoseMemo,
}

impl MoverHistoryBuffer {
    fn clear(&mut self) {
        self.samples.clear();
        self.generation = self.generation.wrapping_add(1);
        self.replay = replay_poses::ReplayPoseMemo::default();
    }

    pub(crate) fn record(&mut self, mover_id: u32, sample: MoverHistorySample) {
        self.generation = self.generation.wrapping_add(1);
        let samples = self.samples.entry(mover_id).or_default();
        if samples
            .back()
            .is_some_and(|existing| existing.server_tick == sample.server_tick)
        {
            samples.pop_back();
        }
        samples.push_back(sample);
        while samples.len() > MOVER_HISTORY_LIMIT {
            samples.pop_front();
        }
    }

    #[cfg(test)]
    pub(crate) fn sample_count(&self, mover_id: u32) -> usize {
        self.samples.get(&mover_id).map_or(0, VecDeque::len)
    }

    /// Reference extrapolation the replay memo must reproduce exactly; see
    /// `replay_poses`. Production replay reads `replay_pose` instead; this stays
    /// as the oracle the memo and snapshot tests compare against.
    #[cfg(test)]
    pub(crate) fn pose_at_tick(
        &self,
        mover_id: u32,
        server_tick: u32,
        tick_dt: f32,
    ) -> Option<MoverPose> {
        let samples = self.samples.get(&mover_id)?;
        if let Some(exact) = samples
            .iter()
            .rev()
            .find(|sample| sample.server_tick == server_tick)
        {
            return Some(exact.pose);
        }
        let seed = samples
            .iter()
            .rev()
            .find(|sample| tick_le(sample.server_tick, server_tick))
            .or_else(|| samples.front())?;
        let mut phase = seed.phase.clone();
        let mut transform = seed.pose.transform;
        let mut pose = seed.pose;
        let advance_ticks = ticks_between(seed.server_tick, server_tick);
        for _ in 0..advance_ticks {
            pose = advance_mover_phase_one_tick(&mut phase, &mut transform, tick_dt);
        }
        Some(pose)
    }
}

impl MoverPoseSource for MoverHistoryBuffer {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        self.samples
            .get(&mover_id)
            .and_then(|samples| samples.back())
            .map(|sample| sample.pose)
    }
}

/// The authoritative local-pawn record this snapshot delivered, captured for the
/// caller to drive reconciliation (M15 Phase 3 Task 5). `ClientReplication` knows
/// which record is `local_player` but does not own `ClientPrediction`; it surfaces
/// the authoritative pose + movement subset + command ack here, and the engine glue
/// (`client_receive_and_apply`, which owns both halves) runs the reconcile. The
/// `Transform` is still applied to the registry in `apply_components_to` so a
/// not-yet-armed local pawn has a pose; reconcile then merges/replays on top.
#[derive(Debug, Clone)]
pub(crate) struct LocalReconcileInput {
    /// The `NetworkId` of the local pawn record. The reconcile path matches by
    /// `entity_id` (the armed pawn's mapped id). Diagnostic/future-use field;
    /// not currently consumed.
    #[allow(dead_code)]
    pub(crate) network_id: NetworkId,
    /// The mapped `EntityId` the record applied to.
    pub(crate) entity_id: EntityId,
    /// The server tick that stamped the authoritative baseline.
    pub(crate) server_tick: u32,
    /// The authoritative pose the host resolved for this pawn. Restored verbatim,
    /// then the unacked commands replay on top.
    pub(crate) transform: Transform,
    /// The authoritative mutable movement-tick subset. Merged onto the EXISTING
    /// descriptor-derived component via `merge_wire_into_movement_state_checked` (never
    /// reconstructs a component). `None` if the local record carried no movement
    /// payload (defensive — wire validation pairs `local_player` with movement).
    pub(crate) movement: Option<WirePlayerMovementState>,
    /// The latest client command tick the host resolved for this pawn before
    /// snapshotting, or `None` if it has resolved none yet. `Some` ⇒ prune history
    /// through it and replay the rest; `None` after prediction has started ⇒
    /// authoritative reset (clear history, apply baseline, do NOT prune by tick).
    pub(crate) acked_tick: Option<u32>,
}

/// A non-local, descriptor-class-bearing entity an `apply_snapshot` first spawned this
/// snapshot, surfaced for the caller to materialize its client-side presentation. The
/// `ClientReplication` spawns the entity Transform-only and maps its `NetworkId` (so
/// it joins the remote interpolation path), but descriptor tables are not in scope
/// here — the net-facing apply is descriptor-blind. The caller
/// (`client_receive_and_apply`, where the descriptor table is in scope) attaches the
/// descriptor display set: Mesh, Light, Emitter, and query-only HitVolume when
/// authored. This descriptor display materialization does not attach `Brain`,
/// `Agent`, `Health`, `Weapon`, or `PlayerMovement`; HitVolume is query-only client
/// presentation derived from authored hit data.
///
/// Surfaced on the unmapped first-spawn of a non-local record carrying an
/// `entity_class`, and on later records while descriptor presentation is incomplete.
/// Completion is cached by `NetworkId`, so a successfully materialized meshless
/// descriptor does not retry, while a missing or unknown descriptor remains
/// retryable. Player and projectile copies retain their separate mesh-based
/// completion rules. Active-weapon attachment changes can also request an update.
/// The local predicted pawn is excluded: it has its own `armed_local_pawn`
/// movement-path materialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteEntityMaterialize {
    /// Host-assigned identity for the mapped remote entity. The descriptor-aware
    /// receive glue uses this to cache its immutable locomotion reference data.
    pub(crate) network_id: NetworkId,
    /// The mapped `EntityId` the spawn produced (Transform-only at this point).
    pub(crate) entity_id: EntityId,
    /// The descriptor class the host stamped on the wire. The caller resolves this to
    /// a descriptor and attaches its mesh. This is the replicated semantic
    /// representation, not necessarily the canonical descriptor name. An unregistered
    /// class leaves the entity transform-only (logged, not rejected).
    pub(crate) entity_class: String,
    /// Optional current mesh-animation state carried by the spawn baseline. It is
    /// applied after descriptor mesh materialization so a client joining an already
    /// active enemy does not miss the initial non-default animation state.
    pub(crate) initial_animation_state: Option<String>,
    /// A changed shared-visible active-weapon archetype, including an initial
    /// unarmed `None`, requires the descriptor-aware caller to refresh the dynamic
    /// `hand_r` attachment after it materializes the mesh.
    pub(crate) active_weapon_archetype: Option<String>,
    /// Distinguishes an attachment update to `None` (clear the hand socket) from a
    /// normal mesh-materialization request with no weapon state change.
    pub(crate) weapon_attachment_changed: bool,
}

/// An applied local-player active-weapon update. The local pawn's body is not a
/// `remote_entities` materialization request, but its shadow-only third-person mesh
/// must receive the same dynamic `hand_r` attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalWeaponAttachmentUpdate {
    pub(crate) entity_id: EntityId,
    pub(crate) active_weapon_archetype: Option<String>,
}

/// A projectile-presentation entity retired by a contact despawn tombstone. The
/// descriptor-aware caller resolves the retained class and spawns local cosmetic
/// impact work after snapshot apply.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RetiredProjectilePresentation {
    pub(crate) entity_class: String,
    pub(crate) transform: Transform,
}

/// The local-pawn baseline an `apply_snapshot` armed this snapshot (M15 Phase 3): the
/// recipient-local `NetworkId` the host flagged `local_player: true`, the `EntityId` it
/// mapped to, and the descriptor `entity_class` the host materialized the pawn from
/// (Task 7). The engine glue (`client_receive_and_apply`) hands `(network_id,
/// entity_id)` to `ClientPrediction::arm` and uses `entity_class` to materialize the
/// matching descriptor-backed `PlayerMovementComponent` on the freshly-spawned (or
/// re-armed) local pawn — so the wire movement subset has something to merge onto and
/// prediction/reconciliation become live. `entity_class` is `None` when the host
/// stamped no class (defensive — the glue then defaults to `"player"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArmedLocalPawn {
    pub(crate) network_id: NetworkId,
    pub(crate) entity_id: EntityId,
    pub(crate) entity_class: Option<String>,
}

/// What an `apply_snapshot` call produced: the ack to send (if the snapshot was
/// accepted), the refresh requests triggered this snapshot, and any typed
/// ignored-payload diagnostics. The caller (engine glue) converts these into
/// `ClientMessage`s and sends them on `Channel::Input`.
#[derive(Debug, Default)]
pub(crate) struct ApplyOutcome {
    /// Whether snapshot-wide state and clocks may advance. A late first entity
    /// baseline can produce an ack while this remains false.
    pub(crate) accepted_newest_snapshot: bool,
    /// The ack for accepted progress, or `None` for an ignored snapshot.
    /// Per-entity entries acknowledge applied baselines, deliberately
    /// rejected presentation bindings, and despawn tombstones. A delta with no held
    /// baseline, or a regressed one, requests repair without acknowledging its
    /// entity baseline.
    pub(crate) ack: Option<AckMessage>,
    /// Written state applies after the entire frame batch has materialized.
    pub(crate) presentation_payloads: Vec<(NetworkId, ComponentPayload)>,
    /// Refresh requests triggered by this snapshot (deltas with no held baseline or
    /// a regressed one, stale mappings). Sent immediately; the pending set also
    /// resends them on cadence.
    pub(crate) refresh_requests: Vec<BaselineRefreshRequest>,
    /// Typed diagnostics for payloads received but deliberately not applied.
    pub(crate) ignored: Vec<IgnoredPayload>,
    /// The local-pawn baseline this snapshot applied (M15 Phase 3 Task 3): the
    /// `NetworkId` the host flagged `local_player: true`, the `EntityId` it mapped to,
    /// and the descriptor `entity_class` to materialize (Task 7), set once
    /// `apply_snapshot` has applied the baseline AND marked the pawn via
    /// `EntityRegistry::mark_local_player_pawn`. The caller hands `(network_id,
    /// entity_id)` to `ClientPrediction::arm` and materializes the component from
    /// `entity_class`. `None` when no local-player baseline was applied this snapshot.
    pub(crate) armed_local_pawn: Option<ArmedLocalPawn>,
    /// The authoritative local-pawn record this snapshot applied (M15 Phase 3
    /// Task 5), for the caller to drive reconciliation. `None` when no local-player
    /// record was applied this snapshot. Captured for EVERY applied local record (a
    /// full baseline or a delta), not only the arming one — reconcile runs on every
    /// authoritative local update; the arming case is just the first.
    pub(crate) local_reconcile: Option<LocalReconcileInput>,
    /// Non-local, descriptor-class-bearing entities whose display presentation needs
    /// materialization or an attachment update. Usually one entry per spawn; mapped
    /// records surface another entry when descriptor presentation is incomplete.
    /// Completion is identity-scoped and independent of Mesh presence, allowing a
    /// complete meshless descriptor copy to stop retrying. Player and projectile
    /// copies use their separate mesh completion rules. Descriptor lookup deliberately
    /// does not happen here because tables are not in scope in this descriptor-blind
    /// apply path.
    pub(crate) remote_entities: Vec<RemoteEntityMaterialize>,
    /// Active-weapon updates for the client-local pawn. Remote updates ride their
    /// corresponding [`remote_entities`](Self::remote_entities) entry so descriptor
    /// mesh materialization and attachment replacement remain ordered together.
    pub(crate) local_weapon_attachments: Vec<LocalWeaponAttachmentUpdate>,
    /// Contact retirements for descriptor-backed projectile presentations.
    pub(crate) retired_projectile_presentations: Vec<RetiredProjectilePresentation>,
    /// Per-snapshot mover correction magnitudes in metres, surfaced so harnesses
    /// can assert corrections stay bounded and non-accumulating.
    pub(crate) mover_corrections: Vec<MoverCorrection>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoverCorrection {
    pub network_id: NetworkId,
    pub mover_id: u32,
    pub magnitude: f32,
    /// Authoritative/predicted-to-target-tick mover state that replaced the
    /// client's live phase. App refreshes its live pose table from this before
    /// input carry or render presentation can consume stale angular motion.
    pub authoritative_state: crate::kinematic_mover::MoverTickState,
}

impl ClientReplication {
    /// Generation changes accepted since the last call, for observer arrival
    /// cues: one per accepted sample whose generation changed, in arrival order
    /// per pawn. Allocates nothing when none arrived.
    pub fn take_observed_teleports(&mut self) -> Vec<ObservedTeleport> {
        let mappings = &self.map;
        self.interp
            .take_observed_teleports(|network_id| mappings.get(&network_id).copied())
    }
    /// Estimate the host script clock from the accepted snapshot anchor and RTT.
    /// Shared presentation state waits for the same clock readiness as bindings.
    #[must_use]
    pub fn presentation_host_time(
        &self,
        local_script_time: f64,
        rtt_micros: Option<f64>,
    ) -> Option<f64> {
        self.bindings
            .presentation_host_time(local_script_time, rtt_micros)
    }

    /// Estimate the host's level tick now, the clock a `levelLoad` `wait`
    /// tail lands on: the accepted snapshot's level tick plus the server
    /// ticks elapsed since it, by `estimated_server_tick` (the time-sync
    /// estimate of the host's current tick). `None` until both exist.
    #[must_use]
    pub fn host_level_tick(&self, estimated_server_tick: Option<f64>) -> Option<f64> {
        self.bindings.host_level_tick(estimated_server_tick)
    }

    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Clear registry-backed replication state for a connected-client level unload
    /// while preserving connection-level snapshot ordering. Returns immediate refresh
    /// requests for every previously-known `NetworkId` so the server re-sends full
    /// baselines for unchanged, already-acked entities after the client registry is
    /// cleared.
    pub(crate) fn reset_for_level_unload(&mut self) -> Vec<BaselineRefreshRequest> {
        self.late_baselines.reset(self.latest_sequence);
        let snapshot_sequence = self.latest_sequence.unwrap_or(0);
        let known: Vec<(NetworkId, u32)> = self
            .map
            .keys()
            .copied()
            .map(|network_id| {
                (
                    network_id,
                    self.baselines.get(&network_id).copied().unwrap_or(0),
                )
            })
            .collect();

        self.map.clear();
        self.reverse_map.clear();
        self.baselines.clear();
        self.active_weapon_archetypes.clear();
        self.remote_entity_classes.clear();
        self.projectile_presentations.clear();
        self.pending_repairs.clear();
        self.interp = RemoteInterpolationBuffer::default();
        self.remote_enemy_walk_playback.clear();
        self.remote_presentation_complete.clear();
        self.remote_player_locomotion.clear();
        self.presented_player_inputs = ClientPresentationInputs::default();
        self.local_pawn = None;
        self.local_pawn_entity_class = None;
        self.unmerged_refresh_sent = None;
        self.bindings.mover_network_ids.clear();
        self.bindings.region_network_ids.clear();
        self.bindings.clear_presentation();
        self.mover_history.clear();
        self.ghost_link_updates.clear();
        self.ghost_audio = None;
        self.presented_state_ticks.clear();

        let mut requests = Vec::with_capacity(known.len());
        for (network_id, missing_baseline_ref) in known {
            self.queue_repair(
                &mut requests,
                snapshot_sequence,
                network_id,
                missing_baseline_ref,
                REFRESH_REASON_LEVEL_RELOAD,
            );
        }
        requests
    }

    /// Retire every identity in the replication map during content demotion. This
    /// despawns replication-created copies and only unbinds authored presentation
    /// entities. Unlike a level unload the registry remains populated, so spawned
    /// copies must be removed before the ordinary clear-only reset.
    pub(crate) fn despawn_all_mapped(&mut self, registry: &mut EntityRegistry) {
        let network_ids: Vec<NetworkId> = self.map.keys().copied().collect();
        for network_id in network_ids {
            let _ = self.apply_despawn(registry, network_id, 0);
        }
    }

    /// Clear demoted-client replication bookkeeping without queueing baseline repair
    /// requests. The host intentionally sends no snapshots until participation returns.
    pub(crate) fn reset_for_demotion(&mut self) {
        self.late_baselines.reset(self.latest_sequence);
        self.map.clear();
        self.reverse_map.clear();
        self.baselines.clear();
        self.active_weapon_archetypes.clear();
        self.remote_entity_classes.clear();
        self.projectile_presentations.clear();
        self.pending_repairs.clear();
        self.interp = RemoteInterpolationBuffer::default();
        self.remote_enemy_walk_playback.clear();
        self.remote_presentation_complete.clear();
        self.remote_player_locomotion.clear();
        self.presented_player_inputs = ClientPresentationInputs::default();
        self.local_pawn = None;
        self.local_pawn_entity_class = None;
        self.unmerged_refresh_sent = None;
        self.bindings.mover_network_ids.clear();
        self.bindings.region_network_ids.clear();
        self.bindings.clear_presentation();
        self.mover_history.clear();
        self.ghost_link_updates.clear();
        self.ghost_audio = None;
        self.presented_state_ticks.clear();
    }

    /// Read-only view of the current `NetworkId -> EntityId` map. Test-only; the
    /// dev-tools overlay uses `remote_debug_entity_ids` so it can exclude the local
    /// predicted pawn.
    #[cfg(test)]
    pub(crate) fn map(&self) -> &HashMap<NetworkId, EntityId> {
        &self.map
    }

    /// Map `entity_id` as this client's armed local pawn without a snapshot
    /// (tests that drive the endpoint's tuning install directly).
    #[cfg(test)]
    pub(crate) fn arm_local_pawn_for_test(&mut self, network_id: NetworkId, entity_id: EntityId) {
        self.map.insert(network_id, entity_id);
        self.reverse_map.insert(entity_id, network_id);
        self.local_pawn = Some(network_id);
    }

    /// Resolve a client-local entity to the host-assigned `NetworkId`, if this
    /// entity is currently mapped by replication.
    pub fn network_id_for_entity(&self, entity_id: EntityId) -> Option<NetworkId> {
        self.reverse_map.get(&entity_id).copied()
    }

    /// The host-assigned id of this client's owned pawn, once the `local_player`
    /// baseline has armed prediction.
    pub(crate) fn local_pawn_network_id(&self) -> Option<NetworkId> {
        self.local_pawn
    }

    /// The two independently-arriving inputs needed to rebuild local movement
    /// when host tuning arrives after the local-player baseline.
    pub(crate) fn armed_local_pawn(&self) -> Option<ArmedLocalPawn> {
        let network_id = self.local_pawn?;
        Some(ArmedLocalPawn {
            network_id,
            entity_id: self.map.get(&network_id).copied()?,
            entity_class: self.local_pawn_entity_class.clone(),
        })
    }

    /// Shared-visible active weapon for the recipient-local pawn. Connected clients
    /// have no host-side weapon entity or `WeaponOwners` entry, so first-person
    /// presentation resolves the descriptor directly from this replicated identity.
    pub fn local_active_weapon_archetype(&self) -> Option<&str> {
        let local_pawn = self.local_pawn?;
        self.active_weapon_archetypes
            .get(&local_pawn)
            .and_then(|archetype| archetype.as_deref())
    }

    /// Active weapon identity received in this client's snapshots for a pawn.
    /// This reads replicated state, including remote pawns observed by this peer.
    pub fn observed_active_weapon_archetype(&self, network_id: NetworkId) -> Option<&str> {
        self.active_weapon_archetypes
            .get(&network_id)
            .and_then(|archetype| archetype.as_deref())
    }

    /// Resolve a replicated `NetworkId` to the current client-local entity.
    /// Host-pushed presentation facts use this after snapshot apply.
    pub(crate) fn entity_for_network_id(&self, network_id: NetworkId) -> Option<EntityId> {
        self.map.get(&network_id).copied()
    }

    /// Frozen visual facts retained with the mapped projectile's baseline.
    pub(crate) fn projectile_presentation(
        &self,
        network_id: NetworkId,
    ) -> Option<postretro_net::wire::WireProjectilePresentation> {
        self.projectile_presentations.get(&network_id).copied()
    }

    fn retain_projectile_presentation(
        &mut self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        facts: Option<postretro_net::wire::WireProjectilePresentation>,
    ) {
        if let Some(facts) = facts {
            self.projectile_presentations.insert(network_id, facts);
            if let Some(id) = self.entity_for_network_id(network_id) {
                super::projectile_presentation::apply_projectile_presentation_facts(
                    registry, id, facts,
                );
            }
        } else {
            self.projectile_presentations.remove(&network_id);
        }
    }

    /// Descriptor class for a mapped remote presentation entity.
    pub(crate) fn remote_entity_class(&self, network_id: NetworkId) -> Option<&str> {
        self.remote_entity_classes.get(&network_id).map(|class| {
            super::descriptor_class::decode_replicated_descriptor_class(class).canonical_name()
        })
    }

    /// Every mapped remote presentation entity with the replicated class it was
    /// materialized from. Display copies carry no provenance, so this is how
    /// client-side presentation tells a remote pawn from a descriptor entity or a
    /// projectile mirror. Allocation-free; unordered.
    pub fn remote_entities_by_class(
        &self,
    ) -> impl Iterator<
        Item = (
            EntityId,
            super::descriptor_class::ReplicatedDescriptorClass<'_>,
        ),
    > + '_ {
        self.remote_entity_classes
            .iter()
            .filter_map(|(network_id, class)| {
                Some((
                    self.entity_for_network_id(*network_id)?,
                    super::descriptor_class::decode_replicated_descriptor_class(class),
                ))
            })
    }

    /// Entity ids that should be drawn as remote debug markers. The local predicted
    /// pawn is mapped, but it is not remote: drawing it here duplicates the player's
    /// own capsule at the reconciled/predicted seam and makes the dev-tools overlay
    /// look like it is vibrating ahead of the camera.
    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn remote_debug_entity_ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.map.iter().filter_map(|(&network_id, &entity_id)| {
            (self.local_pawn != Some(network_id)).then_some(entity_id)
        })
    }

    /// Apply one validated snapshot. Rejects equal-sequence duplicates. Older
    /// snapshots may establish eligible first entity baselines; their other records
    /// are skipped, with tombstones remembered to prevent resurrection. Only newer
    /// snapshots advance the sequence, tick and presentation clock. Returns additive
    /// progress acknowledgements at the newest cursor, refresh requests and ignored
    /// diagnostics, mutating the registry through game-logic-owned primitives.
    #[cfg(test)]
    pub(crate) fn apply_snapshot(
        &mut self,
        registry: &mut EntityRegistry,
        snapshot: &SnapshotMessage,
    ) -> ApplyOutcome {
        self.apply_snapshot_with_mover_target_tick(
            registry,
            snapshot,
            snapshot.server_tick,
            DEFAULT_MOVER_TICK_DT,
        )
    }

    #[cfg(test)]
    pub(crate) fn apply_snapshot_with_mover_target_tick(
        &mut self,
        registry: &mut EntityRegistry,
        snapshot: &SnapshotMessage,
        mover_target_tick: u32,
        mover_tick_dt: f32,
    ) -> ApplyOutcome {
        self.apply_snapshot_with_presentation(
            registry,
            snapshot,
            mover_target_tick,
            mover_tick_dt,
            &mut NoPresentationBindings,
            0.0,
        )
    }

    pub(crate) fn apply_presentation_batch(
        &mut self,
        registry: &mut EntityRegistry,
        collected: Vec<(NetworkId, ComponentPayload)>,
        local_script_time: f64,
        rtt_micros: Option<f64>,
        resolver: &mut dyn PresentationBindingResolver,
    ) -> usize {
        self.bindings.apply_presentation_batch(
            registry,
            &self.map,
            collected,
            local_script_time,
            rtt_micros,
            resolver,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply_snapshot_with_presentation(
        &mut self,
        registry: &mut EntityRegistry,
        snapshot: &SnapshotMessage,
        mover_target_tick: u32,
        mover_tick_dt: f32,
        resolver: &mut dyn PresentationBindingResolver,
        local_script_time: f64,
    ) -> ApplyOutcome {
        if self.latest_sequence == Some(snapshot.sequence) {
            return ApplyOutcome::default();
        }
        let accepted_newest_snapshot = self
            .latest_sequence
            .is_none_or(|latest| snapshot.sequence > latest);
        if accepted_newest_snapshot {
            self.latest_sequence = Some(snapshot.sequence);
            self.acked_server_tick = snapshot.server_tick;
            self.bindings
                .accept_presentation_clock(snapshot.server_script_time, local_script_time);
            self.bindings
                .accept_level_tick(snapshot.level_tick, snapshot.server_tick);
            self.receive_ghost_audio(snapshot.ghost_audio);
        }

        let mut outcome = ApplyOutcome {
            accepted_newest_snapshot,
            ..ApplyOutcome::default()
        };
        // Applied records and terminal tombstones are acked. Rejected presentation
        // bindings are also acked so the host can retire them; deltas with no held
        // baseline or a regressed one request repair without an entity-baseline ack.
        let mut acked_baselines: Vec<(u32, u32)> = Vec::new();
        let mut acked_tombstones: Vec<(u32, u32)> = Vec::new();

        for record in &snapshot.records {
            // A newer empty snapshot can overtake the sole in-flight baseline.
            // Recover only first identities; never replay known state or let a
            // different late identity displace an authored binding/local pawn.
            if !accepted_newest_snapshot
                && !self.can_apply_late_baseline(registry, snapshot.sequence, record, resolver)
            {
                if let EntityRecord::Despawn { network_id, .. } = record {
                    // Even an overtaken tombstone proves this never-recycled id
                    // retired. It cannot mutate newer state, but must prevent a
                    // still-earlier first baseline from resurrecting an unseen id.
                    self.remember_identity(NetworkId(*network_id));
                }
                continue;
            }
            match record {
                EntityRecord::FullBaseline {
                    network_id,
                    baseline_id,
                    components,
                    local_player,
                    last_processed_client_tick,
                    entity_class,
                    active_weapon_archetype,
                    projectile_presentation,
                    presentation_binding,
                } => {
                    let nid = NetworkId(*network_id);
                    if self.bindings.is_presentation_rejected(nid) {
                        acked_baselines.push((*network_id, *baseline_id));
                        continue;
                    }
                    if let Some(binding) = presentation_binding {
                        let resolved = self.bindings.resolve_presentation_baseline(
                            registry, nid, binding, components, resolver,
                        );
                        if let Some(id) = resolved {
                            if self.map.get(&nid).is_some_and(|existing| *existing != id) {
                                self.clear_identity_state(nid);
                                // Identity cleanup clears the just-resolved binding too.
                                let _ = self.bindings.resolve_presentation_baseline(
                                    registry, nid, binding, components, resolver,
                                );
                            }
                            self.insert_mapping(nid, id);
                            self.baselines.insert(nid, *baseline_id);
                            self.pending_repairs.remove(&nid);
                            collect_presentation_payloads(&mut outcome, nid, components);
                        } else {
                            // Rejection remains remembered until participation resets.
                            self.clear_identity_state(nid);
                            self.pending_repairs.remove(&nid);
                        }
                        acked_baselines.push((*network_id, *baseline_id));
                        continue;
                    }
                    if self.apply_full_baseline(
                        registry,
                        snapshot.sequence,
                        snapshot.server_tick,
                        mover_target_tick,
                        mover_tick_dt,
                        NetworkId(*network_id),
                        *baseline_id,
                        components,
                        *local_player,
                        entity_class.as_deref(),
                        &mut outcome,
                    ) {
                        collect_presentation_payloads(
                            &mut outcome,
                            NetworkId(*network_id),
                            components,
                        );
                        self.retain_projectile_presentation(
                            registry,
                            NetworkId(*network_id),
                            *projectile_presentation,
                        );
                        acked_baselines.push((*network_id, *baseline_id));
                        self.maybe_surface_active_weapon_attachment(
                            NetworkId(*network_id),
                            *local_player,
                            entity_class.as_deref(),
                            active_weapon_archetype.clone(),
                            components,
                            &mut outcome,
                        );
                        self.maybe_arm_local_pawn(
                            registry,
                            NetworkId(*network_id),
                            *local_player,
                            entity_class.clone(),
                            &mut outcome,
                        );
                        self.capture_local_reconcile(
                            NetworkId(*network_id),
                            *local_player,
                            components,
                            snapshot.server_tick,
                            *last_processed_client_tick,
                            &mut outcome,
                        );
                    }
                }
                EntityRecord::Delta {
                    network_id,
                    baseline_ref,
                    new_baseline_id,
                    components,
                    local_player,
                    last_processed_client_tick,
                    entity_class,
                    active_weapon_archetype,
                    projectile_presentation,
                    presentation_binding,
                } => {
                    let nid = NetworkId(*network_id);
                    if self.bindings.is_presentation_rejected(nid) {
                        acked_baselines.push((*network_id, *new_baseline_id));
                        continue;
                    }
                    if self.bindings.is_presentation_bound(nid) {
                        if let Some(binding) = presentation_binding
                            && self
                                .bindings
                                .resolve_presentation_baseline(
                                    registry, nid, binding, components, resolver,
                                )
                                .is_none()
                        {
                            self.clear_identity_state(nid);
                            self.pending_repairs.remove(&nid);
                            acked_baselines.push((*network_id, *new_baseline_id));
                            continue;
                        }
                        let live = self.map.get(&nid).is_some_and(|id| registry.exists(*id));
                        let held = self.baselines.get(&nid).copied().filter(|_| live);
                        match held {
                            Some(held) if baseline_is_newer(*new_baseline_id, held) => {
                                self.baselines.insert(nid, *new_baseline_id);
                                collect_presentation_payloads(&mut outcome, nid, components);
                                acked_baselines.push((*network_id, *new_baseline_id));
                            }
                            // A metadata-only resend: a bound authored entity has no
                            // metadata consumers.
                            Some(held) if *new_baseline_id == held => {}
                            // No held baseline or live mapping, or a regressed id.
                            _ => self.queue_repair(
                                &mut outcome.refresh_requests,
                                snapshot.sequence,
                                nid,
                                *baseline_ref,
                                REFRESH_REASON_UNKNOWN_BASELINE,
                            ),
                        }
                        continue;
                    }
                    let disposition = self.apply_delta(
                        registry,
                        snapshot.sequence,
                        snapshot.server_tick,
                        mover_target_tick,
                        mover_tick_dt,
                        nid,
                        *baseline_ref,
                        *new_baseline_id,
                        components,
                        *local_player,
                        entity_class.as_deref(),
                        &mut outcome,
                    );
                    if disposition == DeltaApply::Rejected {
                        continue;
                    }
                    if disposition == DeltaApply::Applied {
                        collect_presentation_payloads(&mut outcome, nid, components);
                        self.retain_projectile_presentation(
                            registry,
                            nid,
                            *projectile_presentation,
                        );
                        acked_baselines.push((*network_id, *new_baseline_id));
                    }
                    // A metadata-only resend still carries the refreshed reconcile
                    // cursor and ownership, so these run for it too.
                    self.maybe_surface_active_weapon_attachment(
                        nid,
                        *local_player,
                        entity_class.as_deref(),
                        active_weapon_archetype.clone(),
                        components,
                        &mut outcome,
                    );
                    self.maybe_arm_local_pawn(
                        registry,
                        nid,
                        *local_player,
                        entity_class.clone(),
                        &mut outcome,
                    );
                    self.capture_local_reconcile(
                        nid,
                        *local_player,
                        components,
                        snapshot.server_tick,
                        *last_processed_client_tick,
                        &mut outcome,
                    );
                }
                EntityRecord::Despawn {
                    network_id,
                    tombstone_id,
                    reason,
                    ..
                } => {
                    if let Some(retired) =
                        self.apply_despawn(registry, NetworkId(*network_id), *reason)
                    {
                        outcome.retired_projectile_presentations.push(retired);
                    }
                    // A despawn always acks its tombstone after retiring the network
                    // identity. The mapped registry entity may have been despawned
                    // (replication-created copy) or preserved and unbound (authored
                    // presentation entity); an unknown identity is already retired.
                    // Acking stops the server resending the tombstone.
                    acked_tombstones.push((*network_id, *tombstone_id));
                }
            }
        }

        if !accepted_newest_snapshot && acked_baselines.is_empty() {
            return outcome;
        }
        // Late entity progress is additive, while the ack's snapshot cursor always
        // names the highest accepted frame. Snapshot-wide lanes use the separate
        // acceptance flag and never replay just because an entity needed this ack.
        outcome.ack = Some(AckMessage {
            latest_snapshot_sequence: self.latest_sequence.expect("a snapshot was accepted"),
            acked_server_tick: self.acked_server_tick,
            entity_baselines: acked_baselines,
            despawn_tombstones: acked_tombstones,
            // State-slot baselines are acked by the Task 3 client apply path; this
            // entity-apply ack carries none.
            slot_baselines: Vec::new(),
            // The faction overlay is acknowledged only after its separate sparse-set
            // apply succeeds in `client_receive_and_apply`.
            faction_sentiment_baseline: None,
        });
        outcome
    }

    fn can_apply_late_baseline(
        &self,
        registry: &EntityRegistry,
        sequence: u32,
        record: &EntityRecord,
        resolver: &dyn PresentationBindingResolver,
    ) -> bool {
        let EntityRecord::FullBaseline {
            network_id,
            components,
            local_player,
            presentation_binding,
            ..
        } = record
        else {
            return false;
        };
        let network_id = NetworkId(*network_id);
        if !self.late_baselines.permits(sequence, network_id)
            || self.map.contains_key(&network_id)
            || self.bindings.is_presentation_rejected(network_id)
            || (*local_player && self.local_pawn.is_some())
        {
            return false;
        }
        // Resolve without touching binding/rejection caches. The ordinary apply
        // path still performs all identity, origin and component validation.
        let target = if let Some(binding) = presentation_binding {
            let kind = match binding.kind {
                0 => PresentationBindingKind::MapLight,
                1 => PresentationBindingKind::FogVolume,
                2 => PresentationBindingKind::MapEntity,
                _ => return false,
            };
            resolver
                .resolve(
                    registry,
                    PresentationBindingKey {
                        kind,
                        index: binding.index,
                    },
                )
                .map(|(entity, _)| entity)
        } else {
            teleporters::first_state(components)
                .and_then(|state| teleporters::find_loaded(registry, state))
                .or_else(|| {
                    ghost_links::first_state(components)
                        .and_then(|state| ghost_links::find_loaded(registry, state))
                })
                .or_else(|| {
                    first_mover_state(components).and_then(|state| {
                        bindings::find_loaded_mover_entity(registry, state.mover_id)
                    })
                })
        };
        target.is_none_or(|entity| !self.reverse_map.contains_key(&entity))
    }

    fn remember_identity(&mut self, network_id: NetworkId) {
        if let Some(sequence) = self.latest_sequence {
            self.late_baselines.remember(network_id, sequence);
        }
    }

    /// Apply a `FullBaseline`. Returns `true` if it applied (and should be acked),
    /// `false` if it requested a refresh instead (stale mapping) or was invalid (no
    /// Transform). See the module state machine.
    #[allow(clippy::too_many_arguments)]
    fn apply_full_baseline(
        &mut self,
        registry: &mut EntityRegistry,
        sequence: u32,
        server_tick: u32,
        mover_target_tick: u32,
        mover_tick_dt: f32,
        network_id: NetworkId,
        baseline_id: u32,
        components: &[ComponentPayload],
        local_player: bool,
        entity_class: Option<&str>,
        outcome: &mut ApplyOutcome,
    ) -> bool {
        match self.map.get(&network_id).copied() {
            // Mapped and live: replace the baseline and update components in place,
            // no respawn. This is the steady-state full-baseline (a refresh response,
            // or a periodic re-baseline).
            Some(existing) if registry.exists(existing) => {
                if !self.apply_components_to(
                    registry,
                    network_id,
                    server_tick,
                    mover_target_tick,
                    mover_tick_dt,
                    existing,
                    components,
                    outcome,
                ) {
                    return false;
                }
                self.maybe_surface_remote_entity_materialize(
                    existing,
                    local_player,
                    entity_class,
                    components,
                    outcome,
                );
                self.baselines.insert(network_id, baseline_id);
                self.pending_repairs.remove(&network_id);
                true
            }
            // Mapped but the entity is stale/missing: the map is corrupt for this id.
            // Drop every cache owned by the stale identity before repair can respawn
            // the same NetworkId. Unrelated registry state remains untouched. Not acked.
            Some(_) => {
                self.clear_identity_state(network_id);
                self.queue_repair(
                    &mut outcome.refresh_requests,
                    sequence,
                    network_id,
                    baseline_id,
                    REFRESH_REASON_STALE_MAPPING,
                );
                false
            }
            // Unmapped: a spawn. Requires a Transform to seed the entity; a baseline
            // without one is invalid and does not spawn.
            None => {
                if let Some(state) = ghost_links::first_state(components) {
                    return self.bind_ghost_link_baseline(
                        registry,
                        network_id,
                        baseline_id,
                        state,
                        server_tick,
                        mover_target_tick,
                        mover_tick_dt,
                        components,
                        outcome,
                    );
                }
                if let Some(state) = teleporters::first_state(components) {
                    return self.bind_teleporter_baseline(
                        registry,
                        network_id,
                        baseline_id,
                        state,
                        server_tick,
                        mover_target_tick,
                        mover_tick_dt,
                        components,
                        outcome,
                    );
                }
                if let Some(mover_state) = first_mover_state(components) {
                    let Some(id) =
                        bindings::find_loaded_mover_entity(registry, mover_state.mover_id)
                    else {
                        log::warn!(
                            "[Net] full baseline for {network_id:?} names unknown mover_id {}; not binding",
                            mover_state.mover_id
                        );
                        outcome.ignored.push(IgnoredPayload::UnknownKinematicMover {
                            network_id: network_id.0,
                            mover_id: mover_state.mover_id,
                        });
                        return false;
                    };
                    self.insert_mapping(network_id, id);
                    self.baselines.insert(network_id, baseline_id);
                    self.pending_repairs.remove(&network_id);
                    self.bindings
                        .mover_network_ids
                        .insert(network_id, mover_state.mover_id);
                    self.interp.forget(network_id);
                    if !self.apply_components_to(
                        registry,
                        network_id,
                        server_tick,
                        mover_target_tick,
                        mover_tick_dt,
                        id,
                        components,
                        outcome,
                    ) {
                        self.remove_mapping(network_id);
                        self.baselines.remove(&network_id);
                        self.bindings.mover_network_ids.remove(&network_id);
                        self.bindings.region_network_ids.remove(&network_id);
                        return false;
                    }
                    return true;
                }
                let Some(spawn_transform) = first_transform(components) else {
                    log::warn!(
                        "[Net] full baseline for {network_id:?} has no Transform; not spawning"
                    );
                    return false;
                };
                let id = registry.spawn(spawn_transform);
                self.insert_mapping(network_id, id);
                self.baselines.insert(network_id, baseline_id);
                self.pending_repairs.remove(&network_id);
                // Apply the remaining (non-Transform) payloads onto the fresh entity.
                if !self.apply_components_to(
                    registry,
                    network_id,
                    server_tick,
                    mover_target_tick,
                    mover_tick_dt,
                    id,
                    components,
                    outcome,
                ) {
                    let _ = registry.despawn(id);
                    self.remove_mapping(network_id);
                    self.baselines.remove(&network_id);
                    return false;
                }
                // E10 Task 6: a non-local baseline carrying a descriptor class is a
                // remote entity. Surface a presentation-materialization request for the
                // caller (descriptor tables are not in scope here). Later mapped
                // records retry while descriptor presentation is incomplete, as
                // reported by the identity-scoped completion cache; this includes
                // missing non-mesh components and unknown descriptors. The local pawn
                // is excluded: its mesh presentation rides `armed_local_pawn` on the
                // movement path.
                if !local_player && let Some(class) = entity_class {
                    outcome.remote_entities.push(RemoteEntityMaterialize {
                        network_id,
                        entity_id: id,
                        entity_class: class.to_string(),
                        initial_animation_state: first_mesh_animation_state(components),
                        active_weapon_archetype: None,
                        weapon_attachment_changed: false,
                    });
                }
                true
            }
        }
    }

    /// Apply a `Delta` and report how it was disposed of. See the module state
    /// machine.
    #[allow(clippy::too_many_arguments)]
    fn apply_delta(
        &mut self,
        registry: &mut EntityRegistry,
        sequence: u32,
        server_tick: u32,
        mover_target_tick: u32,
        mover_tick_dt: f32,
        network_id: NetworkId,
        baseline_ref: u32,
        new_baseline_id: u32,
        components: &[ComponentPayload],
        local_player: bool,
        entity_class: Option<&str>,
        outcome: &mut ApplyOutcome,
    ) -> DeltaApply {
        // The client must hold some baseline and a live mapped entity. A delta
        // carries the full component set, so any held baseline older than
        // `new_baseline_id` suffices. Deltas reach here only from an accepted newest
        // snapshot, whose ids never run backward, so a regressed id is a broken
        // invariant: skipping it would leave the host resending forever.
        let held = self.baselines.get(&network_id).copied();
        let mapped = self.map.get(&network_id).copied();
        let (held, id) = match (held, mapped) {
            (Some(held), Some(id))
                if registry.exists(id)
                    && (new_baseline_id == held || baseline_is_newer(new_baseline_id, held)) =>
            {
                (held, id)
            }
            _ => {
                self.queue_repair(
                    &mut outcome.refresh_requests,
                    sequence,
                    network_id,
                    baseline_ref,
                    REFRESH_REASON_UNKNOWN_BASELINE,
                );
                return DeltaApply::Rejected;
            }
        };
        if new_baseline_id == held {
            // The host refreshes authority metadata every tick without bumping an
            // unchanged baseline, so this resend's components are already applied.
            // Only the materialize retry runs here; the caller runs the rest.
            self.maybe_surface_remote_entity_materialize(
                id,
                local_player,
                entity_class,
                components,
                outcome,
            );
            return DeltaApply::MetadataRefresh;
        }
        if !self.apply_components_to(
            registry,
            network_id,
            server_tick,
            mover_target_tick,
            mover_tick_dt,
            id,
            components,
            outcome,
        ) {
            return DeltaApply::Rejected;
        }
        self.maybe_surface_remote_entity_materialize(
            id,
            local_player,
            entity_class,
            components,
            outcome,
        );
        // Advance the stored baseline so newest-wins can skip a duplicate and refresh
        // on a regression. An empty-component delta is a valid no-op apply: it still
        // advances the baseline (the server bumped the baseline id even if the
        // mirrors did not change the applied set), so the client stays in step.
        self.baselines.insert(network_id, new_baseline_id);
        DeltaApply::Applied
    }

    fn maybe_surface_remote_entity_materialize(
        &self,
        entity_id: EntityId,
        local_player: bool,
        entity_class: Option<&str>,
        components: &[ComponentPayload],
        outcome: &mut ApplyOutcome,
    ) {
        if local_player {
            return;
        }
        let Some(class) = entity_class else {
            return;
        };
        // Callers only reach this after a record has applied, which means the
        // bidirectional mapping is live. Reading the NetworkId back from that
        // invariant avoids passing a duplicated identity through this helper.
        let Some(&network_id) = self.reverse_map.get(&entity_id) else {
            return;
        };
        if self.remote_presentation_complete.contains(&network_id) {
            return;
        }
        let cached_weapon = self.active_weapon_archetypes.get(&network_id).cloned();
        outcome.remote_entities.push(RemoteEntityMaterialize {
            network_id,
            entity_id,
            entity_class: class.to_string(),
            initial_animation_state: first_mesh_animation_state(components),
            active_weapon_archetype: cached_weapon.clone().flatten(),
            // A recovered mesh has no attachment regardless of whether identity
            // changed. Reapply even cached `None` so binding resolution observes the
            // newly materialized (initially unarmed) presentation surface.
            weapon_attachment_changed: cached_weapon.is_some(),
        });
    }

    /// Compare shared-visible active-weapon identity after a record has applied.
    /// The wire validator already guarantees a non-`None` value only rides a
    /// movement record; the component check below also makes the unarmed `None`
    /// initial population unambiguous without tracking transform-only entities.
    fn maybe_surface_active_weapon_attachment(
        &mut self,
        network_id: NetworkId,
        local_player: bool,
        entity_class: Option<&str>,
        active_weapon_archetype: Option<String>,
        components: &[ComponentPayload],
        outcome: &mut ApplyOutcome,
    ) {
        if !components
            .iter()
            .any(|payload| matches!(payload, ComponentPayload::PlayerMovementState(_)))
        {
            return;
        }

        let changed = match self.active_weapon_archetypes.entry(network_id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(active_weapon_archetype.clone());
                true
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if entry.get() == &active_weapon_archetype {
                    false
                } else {
                    entry.insert(active_weapon_archetype.clone());
                    true
                }
            }
        };
        if !changed {
            return;
        }

        let Some(&entity_id) = self.map.get(&network_id) else {
            return;
        };
        if local_player {
            outcome
                .local_weapon_attachments
                .push(LocalWeaponAttachmentUpdate {
                    entity_id,
                    active_weapon_archetype,
                });
            return;
        }

        let Some(entity_class) = entity_class else {
            return;
        };
        if let Some(remote) = outcome
            .remote_entities
            .iter_mut()
            .find(|remote| remote.entity_id == entity_id)
        {
            remote.active_weapon_archetype = active_weapon_archetype;
            remote.weapon_attachment_changed = true;
            return;
        }
        outcome.remote_entities.push(RemoteEntityMaterialize {
            network_id,
            entity_id,
            entity_class: entity_class.to_string(),
            initial_animation_state: None,
            active_weapon_archetype,
            weapon_attachment_changed: true,
        });
    }

    /// Arm client prediction for the recipient-local movement pawn (M15 Phase 3
    /// Task 3). Called only after a record APPLIED (so the `NetworkId` is mapped and
    /// live). When the host flagged the record `local_player: true`, this marks the
    /// mapped entity as the local player pawn (`mark_local_player_pawn`) and records
    /// the armed `(NetworkId, EntityId)` baseline in the outcome for the caller to
    /// hand to `ClientPrediction::arm`. A no-op for non-local records.
    ///
    /// The client does not drive a provisional local pawn before this fires: arming
    /// requires a stable `NetworkId -> EntityId` mapping (proven by the prior apply)
    /// AND a `local_player` baseline. Task 5 seam: the per-snapshot reconciliation
    /// (merge `PlayerMovementState`, restore Transform, prune-through-ack, replay)
    /// hangs off this same applied-local-record point — this task only marks + arms.
    fn maybe_arm_local_pawn(
        &mut self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        local_player: bool,
        entity_class: Option<String>,
        outcome: &mut ApplyOutcome,
    ) {
        if !local_player {
            return;
        }
        let Some(&entity_id) = self.map.get(&network_id) else {
            // Defensive: an applied record is always mapped. Wire validation already
            // guarantees `local_player: true` only rides a movement record.
            return;
        };
        let mesh_missing = !registry
            .has_component_kind(entity_id, ComponentKind::Mesh)
            .unwrap_or(false);
        if mesh_missing
            && !outcome
                .local_weapon_attachments
                .iter()
                .any(|update| update.entity_id == entity_id)
            && let Some(cached_weapon) = self.active_weapon_archetypes.get(&network_id)
        {
            outcome
                .local_weapon_attachments
                .push(LocalWeaponAttachmentUpdate {
                    entity_id,
                    active_weapon_archetype: cached_weapon.clone(),
                });
        }
        // Mark the mapped entity as the local player pawn so camera follow, the
        // movement-pawn lookup, and prediction all converge on the same EntityId.
        if let Err(err) = registry.mark_local_player_pawn(entity_id) {
            log::warn!("[Net] failed to mark local player pawn for {network_id:?}: {err}");
            return;
        }
        // Record the local pawn so the interp path (record + sample) excludes it: it is
        // prediction-driven, not remote-interpolated. Drop any interp samples already
        // buffered for it (a record applied before the arming snapshot seeded one).
        self.local_pawn = Some(network_id);
        if entity_class.is_some() || self.local_pawn_entity_class.is_none() {
            self.local_pawn_entity_class = entity_class.clone();
        }
        self.interp.forget(network_id);
        self.remote_enemy_walk_playback.remove(&network_id);
        self.remote_player_locomotion.remove(&network_id);
        self.bindings.mover_network_ids.remove(&network_id);
        self.bindings.region_network_ids.remove(&network_id);
        outcome.armed_local_pawn = Some(ArmedLocalPawn {
            network_id,
            entity_id,
            entity_class,
        });
    }

    /// Capture an applied `local_player` record's authoritative state for the caller's
    /// reconcile pass (M15 Phase 3 Task 5). A no-op for a non-local record. The
    /// reconcile orchestration (merge / restore / prune / replay / smooth) lives in
    /// the engine glue that owns both `ClientReplication` and `ClientPrediction`, so
    /// this only surfaces the inputs: the authoritative `Transform`, the mutable
    /// movement-tick subset, and the command ack. Runs AFTER `maybe_arm_local_pawn`,
    /// so a record that armed this frame is also captured (reconcile runs on the
    /// arming snapshot too — restoring the baseline with no unacked tail to replay).
    fn capture_local_reconcile(
        &mut self,
        network_id: NetworkId,
        local_player: bool,
        components: &[ComponentPayload],
        server_tick: u32,
        acked_tick: Option<u32>,
        outcome: &mut ApplyOutcome,
    ) {
        if !local_player {
            return;
        }
        let Some(&entity_id) = self.map.get(&network_id) else {
            return;
        };
        // Pull the authoritative pose + movement subset out of the payloads. A local
        // record is a movement record (wire validation), so both are normally present;
        // the Transform is required for reconcile to restore. No finiteness re-check
        // here: `RawSnapshotMessage::validate` (postretro-net `wire.rs`) already rejects
        // any non-finite `PlayerMovementState` before this typed apply path runs, so a
        // payload that reaches here is finite by construction. Re-checking would only
        // risk a silent partial apply (the Transform merged, the movement dropped).
        let Some(transform) = first_transform(components) else {
            log::warn!(
                "[Net] local_player record for {network_id:?} has no Transform; skipping reconcile"
            );
            return;
        };
        let movement = components.iter().find_map(|p| match p {
            ComponentPayload::PlayerMovementState(m) => Some(m.clone()),
            _ => None,
        });
        outcome.local_reconcile = Some(LocalReconcileInput {
            network_id,
            entity_id,
            server_tick,
            transform,
            movement,
            acked_tick,
        });
    }

    /// Retire a mapped network identity and drop its mapping + baseline. Despawn its
    /// replication-created entity, or only unbind an authored presentation entity.
    /// Idempotent: an unknown or already-retired `NetworkId` is a no-op.
    fn apply_despawn(
        &mut self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        reason: u8,
    ) -> Option<RetiredProjectilePresentation> {
        let retired_projectile = self.map.get(&network_id).and_then(|&id| {
            if reason != super::PROJECTILE_CONTACT_DESPAWN_REASON {
                return None;
            }
            let provenance = registry.get_component::<DescriptorProvenance>(id).ok()?;
            if provenance.spawn_path != DescriptorSpawnPath::ProjectilePresentation {
                return None;
            }
            let transform = *registry.get_component::<Transform>(id).ok()?;
            Some(RetiredProjectilePresentation {
                entity_class: provenance.canonical_name.clone(),
                transform,
            })
        });
        let bound = self.bindings.is_presentation_bound(network_id)
            || self.bindings.region_network_ids.contains_key(&network_id);
        if let Some(id) = self.clear_identity_state(network_id)
            && !bound
        {
            // `despawn` errors on a stale id; the entity may already be gone. Either
            // way the post-state is "despawned", so the error is ignored.
            let _ = registry.despawn(id);
        }
        // Retiring this identity also clears its pending repair; a later baseline can
        // establish a fresh mapping if the identity returns.
        self.pending_repairs.remove(&network_id);
        retired_projectile
    }

    /// Drop every registry-backed cache owned by one network identity. Used by both
    /// ordinary despawn and stale-map repair so a later mapping starts clean.
    fn clear_identity_state(&mut self, network_id: NetworkId) -> Option<EntityId> {
        let entity_id = self.remove_mapping(network_id);
        self.baselines.remove(&network_id);
        self.interp.forget(network_id);
        self.remote_enemy_walk_playback.remove(&network_id);
        self.remote_player_locomotion.remove(&network_id);
        self.presented_player_inputs.aim_pitches.remove(&network_id);
        self.presented_player_inputs
            .heading_yaws
            .remove(&network_id);
        self.active_weapon_archetypes.remove(&network_id);
        self.remote_entity_classes.remove(&network_id);
        self.projectile_presentations.remove(&network_id);
        self.bindings.mover_network_ids.remove(&network_id);
        self.bindings.region_network_ids.remove(&network_id);
        if self.local_pawn == Some(network_id) {
            self.local_pawn = None;
            self.local_pawn_entity_class = None;
            self.unmerged_refresh_sent = None;
        }
        entity_id
    }

    fn insert_mapping(&mut self, network_id: NetworkId, entity_id: EntityId) {
        self.remember_identity(network_id);
        if let Some(previous_entity) = self.map.insert(network_id, entity_id) {
            self.reverse_map.remove(&previous_entity);
            if previous_entity != entity_id {
                self.bindings.forget_presentation_identity(network_id);
                self.remote_presentation_complete.remove(&network_id);
                self.active_weapon_archetypes.remove(&network_id);
            }
        }
        if let Some(previous_network) = self.reverse_map.insert(entity_id, network_id)
            && previous_network != network_id
        {
            self.remember_identity(previous_network);
            self.map.remove(&previous_network);
            self.bindings.forget_presentation_identity(previous_network);
            self.remote_presentation_complete.remove(&previous_network);
            self.active_weapon_archetypes.remove(&previous_network);
            self.remote_entity_classes.remove(&previous_network);
            self.projectile_presentations.remove(&previous_network);
        }
    }

    fn remove_mapping(&mut self, network_id: NetworkId) -> Option<EntityId> {
        self.remember_identity(network_id);
        self.bindings.forget_presentation_identity(network_id);
        self.remote_presentation_complete.remove(&network_id);
        let entity_id = self.map.remove(&network_id)?;
        self.reverse_map.remove(&entity_id);
        self.active_weapon_archetypes.remove(&network_id);
        self.remote_entity_classes.remove(&network_id);
        self.projectile_presentations.remove(&network_id);
        Some(entity_id)
    }

    /// Apply each component payload onto `id`. A `Transform` is written through
    /// `set_component_value` (idempotent — re-applying the spawn Transform is
    /// harmless and keeps this path uniform between spawn and update) and recorded
    /// into the per-entity interpolation buffer stamped by `server_tick`. A
    /// `PlayerMovementState` payload merges only onto an entity that already carries a
    /// local `PlayerMovementComponent`; otherwise the merge is skipped with a typed
    /// diagnostic. Its intrinsic velocity, stance, aim pitch and teleport sample
    /// still ride the interpolation sample, for bounded extrapolation and
    /// remote-player presentation. A remote player's `MeshAnimationState` queues
    /// until the presented pose reaches `server_tick`.
    #[allow(clippy::too_many_arguments)]
    fn apply_components_to(
        &mut self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        server_tick: u32,
        mover_target_tick: u32,
        mover_tick_dt: f32,
        id: EntityId,
        components: &[ComponentPayload],
        outcome: &mut ApplyOutcome,
    ) -> bool {
        // Capture the record's movement presentation (if any) up front: intrinsic
        // velocity, stance, aim pitch and teleport sample. They stamp the
        // Transform sample, velocity for bounded extrapolation, and keep player
        // locomotion separate from mover-carried world-transform displacement.
        let record_movement = components.iter().find_map(|payload| match payload {
            ComponentPayload::PlayerMovementState(m) if payload_is_finite(payload) => Some(m),
            _ => None,
        });
        let record_intrinsic_velocity =
            record_movement.map(|movement| Vec3::from_array(movement.velocity));
        let record_stance = record_movement.map(super::movement_state::wire_locomotion_stance);
        let record_aim_pitch = record_movement.map_or(0.0, |movement| movement.aim_pitch);
        let record_teleport = record_movement.map(|movement| TeleportSample {
            generation: movement.teleport.generation,
            last_teleporter: movement.teleport.last_teleporter,
        });
        let record_has_transform = components
            .iter()
            .any(|payload| matches!(payload, ComponentPayload::Transform(_)));

        // The local predicted pawn is reconcile-driven: its authoritative pose +
        // movement subset are captured by `capture_local_reconcile` and the reconcile
        // path merges/replays them. Once armed, do NOT write its authoritative
        // Transform here: reconcile must still be able to read the pre-apply predicted
        // registry pose to seed the presentation offset. The arming snapshot is the
        // one exception (`local_pawn` is not set until `maybe_arm_local_pawn` runs
        // after this): it may seed the spawn/baseline pose here, and then `forget`
        // drops the one interpolation sample.
        let is_local = self.local_pawn == Some(network_id);
        let bound_mover_id = self.bindings.mover_network_ids.get(&network_id).copied();
        let bound_region = self.bindings.region_network_ids.get(&network_id).copied();
        if let Some(state) = teleporters::first_state(components)
            && (bound_region != Some(RegionBinding::Teleporter(state.index))
                || !teleporters::matches(registry, id, state))
        {
            log::warn!("[Net] rejecting mismatched teleporter binding for {network_id:?}");
            return false;
        }
        if let Some(state) = ghost_links::first_state(components)
            && (bound_region != Some(RegionBinding::GhostLink(state.index))
                || !ghost_links::matches(registry, id, state))
        {
            log::warn!("[Net] rejecting mismatched ghost link binding for {network_id:?}");
            return false;
        }
        if let Some(mover_id) = unknown_ground_mover_id(registry, components) {
            outcome.ignored.push(IgnoredPayload::UnknownGroundMover {
                network_id: network_id.0,
                mover_id,
            });
            return false;
        }
        let mover_state = first_mover_state(components);
        let mover_apply = mover_state.and_then(|wire| {
            prepare_mover_apply(
                registry,
                id,
                network_id,
                bound_mover_id,
                wire,
                server_tick,
                mover_target_tick,
                first_transform(components)?,
                mover_tick_dt,
            )
        });
        let invalid_bound_mover_payload = mover_state.is_some() && mover_apply.is_none();
        if invalid_bound_mover_payload {
            if let Some(wire) = mover_state {
                outcome.ignored.push(IgnoredPayload::UnknownKinematicMover {
                    network_id: network_id.0,
                    mover_id: wire.mover_id,
                });
            }
            return false;
        }

        if let Some(plan) = &mover_apply {
            if let Ok(current) = registry.get_component::<Transform>(id) {
                outcome.mover_corrections.push(MoverCorrection {
                    network_id,
                    mover_id: plan.mover_id,
                    magnitude: (current.position - plan.transform.position).length(),
                    authoritative_state: crate::kinematic_mover::MoverTickState {
                        entity: id,
                        transform: plan.pose.transform,
                        linear_velocity: plan.pose.linear_velocity,
                        tick_delta: plan.pose.tick_delta,
                        angular_velocity: plan.pose.angular_velocity,
                        tick_rotation_delta: plan.pose.tick_rotation_delta,
                        carry_yaw: plan.pose.carry_yaw,
                        tick_dt: plan.pose.tick_dt,
                    },
                });
            }
            let _ = registry.set_component(id, plan.phase.clone());
            let _ = registry.set_component(id, plan.transform);
            self.bindings
                .mover_network_ids
                .insert(network_id, plan.mover_id);
            self.interp.forget(network_id);
            for sample in &plan.history_samples {
                self.mover_history.record(plan.mover_id, sample.clone());
            }
        }

        for payload in components {
            // Untrusted-wire guard: a non-finite pose/velocity is dropped before it
            // reaches the registry, where it would poison interpolation/camera math.
            if !payload_is_finite(payload) {
                log::warn!("[Net] dropping non-finite payload for {network_id:?}");
                continue;
            }
            match payload {
                ComponentPayload::TeleporterState(state) => {
                    teleporters::apply(registry, id, *state)
                }
                // Received link state never writes the installed component, which
                // holds this client's local spawn state; presentation history
                // takes it from the queue instead.
                ComponentPayload::GhostLinkState(state) => {
                    self.queue_ghost_link_update(server_tick, *state)
                }
                ComponentPayload::Transform(wire) => {
                    let transform = wire_to_transform(wire);
                    if !is_local {
                        if mover_apply.is_some()
                            || invalid_bound_mover_payload
                            || bound_mover_id.is_some()
                            || bound_region.is_some()
                        {
                            continue;
                        }
                        let value = ComponentValue::Transform(transform);
                        // The entity is live here (caller checked); the only failure mode
                        // is an unsupported kind, impossible for Transform. This seeds the
                        // initial visible pose; the interpolation sampler drives it after.
                        let _ = registry.set_component_value(id, value);
                    }
                    // Record the server-tick-stamped sample for the interpolation
                    // buffer — skipped for the local pawn (prediction-driven, never
                    // remote-interpolated).
                    if !is_local
                        && mover_apply.is_none()
                        && !invalid_bound_mover_payload
                        && bound_mover_id.is_none()
                        && bound_region.is_none()
                    {
                        self.interp.record(
                            network_id,
                            TransformSample {
                                teleport: record_teleport,
                                server_tick,
                                transform,
                                intrinsic_velocity: record_intrinsic_velocity,
                                stance: record_stance,
                                aim_pitch: record_aim_pitch,
                            },
                        );
                    }
                }
                ComponentPayload::PlayerMovementState(_) => {
                    // Merge ONLY onto an entity that already has a descriptor-derived
                    // PlayerMovementComponent. The wire subset is not a full component
                    // and must never construct one (entity_model.md §7b: movement is
                    // descriptor-owned). A remote pawn has no local source, so the
                    // merge is skipped with a typed diagnostic; its velocity and
                    // stance, captured above, still feed presentation. The local
                    // pawn's subset reaches its component through reconcile.
                    let has_local = registry
                        .has_component_kind(id, ComponentKind::PlayerMovement)
                        .unwrap_or(false);
                    // Pin the wire/engine discriminant equality at the one site that
                    // reasons about this payload kind (drift guard, compiles out in
                    // release).
                    debug_assert_eq!(
                        payload.kind(),
                        COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
                        "movement payload discriminant drifted"
                    );
                    if !has_local && !is_local {
                        outcome
                            .ignored
                            .push(IgnoredPayload::MovementWithoutLocalComponent {
                                network_id: network_id.0,
                            });
                    }
                }
                ComponentPayload::MeshAnimationState(wire) => {
                    // A remote avatar's state describes the body at `server_tick`,
                    // which the interpolation delay has yet to present; it waits
                    // for the presented pose (`sample_into_registry`).
                    let remote_player = (!is_local)
                        .then(|| self.remote_player_locomotion.get_mut(&network_id))
                        .flatten();
                    let switched = match remote_player {
                        Some(locomotion) => {
                            locomotion.queue_correction(server_tick, &wire.current_state);
                            true
                        }
                        None => apply_mesh_animation_state(registry, id, &wire.current_state, true),
                    };
                    if !switched {
                        log::trace!(
                            "[Net] deferred mesh animation state `{}` for {network_id:?}",
                            wire.current_state
                        );
                    }
                }
                ComponentPayload::KinematicMoverState(wire) => {
                    if mover_apply.is_none() {
                        let _ = bindings::validate_kinematic_mover_state_binding(
                            registry,
                            id,
                            network_id,
                            bound_mover_id,
                            wire,
                        );
                    }
                }
                // Presentation state applies after materialization, once the
                // binding and bridge-clock path is installed.
                ComponentPayload::LightState(_)
                | ComponentPayload::FogState(_)
                | ComponentPayload::EmitterState(_) => {}
            }
        }
        if !record_has_transform
            && let Some(intrinsic_velocity) = record_intrinsic_velocity
            && let Some(stance) = record_stance
            && !is_local
            && mover_apply.is_none()
            && !invalid_bound_mover_payload
            && bound_mover_id.is_none()
        {
            self.interp.record_intrinsic_velocity(
                network_id,
                server_tick,
                intrinsic_velocity,
                stance,
                record_aim_pitch,
                record_teleport,
            );
        }
        true
    }

    /// Add `network_id` to the pending-repair set and emit one `BaselineRefreshRequest`
    /// now. The pending entry resends on the 5 Hz cadence until the matching full
    /// baseline clears it. Re-queuing an already-pending entity refreshes its missing
    /// ref/reason and resets its cadence so the immediate request is not double-sent.
    fn queue_repair(
        &mut self,
        requests: &mut Vec<BaselineRefreshRequest>,
        sequence: u32,
        network_id: NetworkId,
        missing_baseline_ref: u32,
        reason: u8,
    ) {
        self.pending_repairs.insert(
            network_id,
            PendingRepair {
                missing_baseline_ref,
                snapshot_sequence: sequence,
                reason,
                since_last_request_ms: 0.0,
            },
        );
        requests.push(BaselineRefreshRequest {
            snapshot_sequence: sequence,
            network_id: network_id.0,
            missing_baseline_ref,
            reason,
        });
    }

    /// Until the armed local pawn's first record has merged, keep a full baseline
    /// of it requested. A record that arrives before the host tuning installs
    /// cannot merge, since reconcile replays with that tuning, and an idle pawn's
    /// later records arrive only while its resolved cursor advances; without a fresh baseline the client would
    /// predict from no teleport record at all.
    ///
    /// `wanted` is false once the record has merged, and while no merge could
    /// succeed (no host tuning, or tuning with no movement leaves the pawn without a
    /// movement component); either way a pending request of this kind is dropped.
    /// Only the first request for a pawn sends now, returned for the caller to
    /// send. The answering full baseline clears the pending entry, so a merge that
    /// still fails re-enters it without sending and the 5 Hz resend cadence paces
    /// the retries: a forced refresh bypasses the join baseline budget, and must
    /// not repeat every round trip. Called once per frame.
    pub(crate) fn refresh_unmerged_local_pawn(
        &mut self,
        wanted: bool,
    ) -> Option<BaselineRefreshRequest> {
        let network_id = self.local_pawn?;
        if !wanted {
            if self
                .pending_repairs
                .get(&network_id)
                .is_some_and(|repair| repair.reason == REFRESH_REASON_UNMERGED_LOCAL_PAWN)
            {
                self.pending_repairs.remove(&network_id);
            }
            return None;
        }
        if self.pending_repairs.contains_key(&network_id) {
            return None;
        }
        let sequence = self.latest_sequence.unwrap_or(0);
        let baseline = self.baselines.get(&network_id).copied().unwrap_or(0);
        if self.unmerged_refresh_sent == Some(network_id) {
            self.pending_repairs.insert(
                network_id,
                PendingRepair {
                    missing_baseline_ref: baseline,
                    snapshot_sequence: sequence,
                    reason: REFRESH_REASON_UNMERGED_LOCAL_PAWN,
                    since_last_request_ms: 0.0,
                },
            );
            return None;
        }
        self.unmerged_refresh_sent = Some(network_id);
        let mut requests = Vec::with_capacity(1);
        self.queue_repair(
            &mut requests,
            sequence,
            network_id,
            baseline,
            REFRESH_REASON_UNMERGED_LOCAL_PAWN,
        );
        requests.pop()
    }

    /// Advance the pending-repair cadence by `dt_ms` and return the refresh requests
    /// due this frame (one per pending entity that has waited a full interval). Called
    /// once per client frame; the matching `FullBaseline` apply removes the entry, so
    /// a satisfied repair stops resending. No-op when nothing is pending.
    pub(crate) fn tick_pending_repairs(&mut self, dt_ms: f32) -> Vec<BaselineRefreshRequest> {
        let mut due = Vec::new();
        for (network_id, repair) in self.pending_repairs.iter_mut() {
            repair.since_last_request_ms += dt_ms;
            if repair.since_last_request_ms >= REPAIR_RESEND_INTERVAL_MS {
                repair.since_last_request_ms = 0.0;
                due.push(BaselineRefreshRequest {
                    snapshot_sequence: repair.snapshot_sequence,
                    network_id: network_id.0,
                    missing_baseline_ref: repair.missing_baseline_ref,
                    reason: repair.reason,
                });
            }
        }
        due
    }

    /// Sample every mapped remote entity's interpolation buffer at the render server
    /// tick `render_server_tick` (already `estimated_server_tick - interpolation_delay`)
    /// and write the resolved pose through the registry's remote-presentation helper.
    ///
    /// Game-logic-owned: runs after this frame's network receive/apply and before the
    /// render collectors read entities (the renderer stays read-only). Each write sets
    /// the entity's visible `Transform` to the freshly-interpolated pose. The buffer
    /// already resolved that pose at the correct *server-time* target, so the
    /// remote-presentation write is *alpha-agnostic*: it collapses previous == current
    /// and the render-stage `interpolated_transform` blend reproduces the pose verbatim
    /// at any frame alpha (the sim sub-tick fraction is an unrelated time base and must
    /// not re-blend an already-resolved pose — see
    /// `EntityRegistry::set_presentation_transform`). An entity with no buffered
    /// samples yet is left at its last-applied pose. Returns presentation stats for
    /// diagnostics and adaptive delay feedback.
    pub(crate) fn sample_into_registry(
        &mut self,
        registry: &mut EntityRegistry,
        render_server_tick: f64,
        frame_anim_time: f64,
    ) -> InterpolationSampleStats {
        let mut stats = InterpolationSampleStats::default();
        // Diagnostics-only: open this render frame's interpolation window at the
        // resolved target tick. No effect on the sampled poses.
        self.netdiag.begin_frame(render_server_tick);
        self.presented_player_inputs.aim_pitches.clear();
        self.presented_player_inputs.heading_yaws.clear();
        self.presented_state_ticks.clear();
        // Collect (network_id, entity_id) first to avoid borrowing `self.map` while
        // writing back through the registry.
        let mapped: Vec<(NetworkId, EntityId)> = self.map.iter().map(|(&n, &e)| (n, e)).collect();
        for (network_id, entity_id) in mapped {
            if !registry.exists(entity_id) {
                continue;
            }
            // The local predicted pawn is prediction/reconcile-driven, not remote-
            // interpolated: skip it so its reconciled pose is not clobbered.
            if self.local_pawn == Some(network_id) {
                continue;
            }
            if (self.bindings.mover_network_ids.contains_key(&network_id)
                || self.bindings.region_network_ids.contains_key(&network_id))
                || self.bindings.is_presentation_bound(network_id)
            {
                continue;
            }
            let Some(pose) = self.interp.presented_pose(network_id, render_server_tick) else {
                continue; // no samples buffered yet
            };
            self.presented_state_ticks
                .insert(network_id, pose.state_tick);
            let mut transform = pose.transform;
            if let Some(facts) = self.projectile_presentations.get(&network_id)
                && facts.sprite_size.is_none()
            {
                transform.scale = Vec3::splat(facts.model_scale);
            }
            let _ = registry.set_presentation_transform(entity_id, transform);
            // Diagnostics-only: record the presented remote pose to detect stepped /
            // low-effective-rate interpolation output (position + orientation).
            self.netdiag.record_remote(
                network_id.0,
                pose.transform.position,
                pose.transform.rotation,
            );
            stats.presented += 1;
            self.update_remote_enemy_walk_playback_rate(
                registry,
                network_id,
                entity_id,
                pose.speed_xz,
                frame_anim_time,
            );
            if self.remote_player_locomotion.contains_key(&network_id) {
                if pose.aim_pitch.is_finite() {
                    self.presented_player_inputs
                        .aim_pitches
                        .insert(network_id, pose.aim_pitch);
                }
                let intrinsic_velocity = pose
                    .intrinsic_velocity
                    .filter(|velocity| velocity.is_finite());
                let (aim_yaw, _, _) = pose.transform.rotation.to_euler(glam::EulerRot::YXZ);
                self.apply_due_remote_player_correction(
                    registry,
                    network_id,
                    entity_id,
                    Some(render_server_tick),
                );
                let direction = self.update_remote_player_locomotion(
                    registry,
                    network_id,
                    entity_id,
                    intrinsic_velocity.unwrap_or(Vec3::ZERO),
                    pose.stance,
                    aim_yaw,
                    frame_anim_time,
                    RemotePlayerPresentation::Interpolated,
                );
                if let Some(velocity) = intrinsic_velocity.filter(is_horizontal_motion) {
                    let heading_yaw =
                        crate::sim::player_travel_heading_yaw(velocity, direction, 0.0);
                    self.presented_player_inputs
                        .heading_yaws
                        .insert(network_id, heading_yaw);
                }
            }
            // Diagnostic: a HeldNewest after sustained starvation is the visible
            // freeze the buffer falls back to; logged sparingly at trace.
            if pose.holds_newest {
                stats.held_newest += 1;
                if self
                    .interp
                    .held_newest_needs_feedback(network_id, self.acked_server_tick)
                {
                    stats.starvation_feedback += 1;
                }
                log::trace!(
                    "[Net] remote {network_id:?} holding last pose (interp buffer starved)"
                );
            }
        }
        stats
    }

    /// Apply held-pose walk playback before time sync can name a render tick.
    /// The baseline pose already visible in the registry is frozen until the
    /// estimator initializes, so its displayed XZ speed is zero.
    pub(crate) fn apply_held_remote_enemy_walk_playback_rates(
        &self,
        registry: &mut EntityRegistry,
        frame_anim_time: f64,
    ) {
        for (&network_id, &entity_id) in &self.map {
            if !registry.exists(entity_id)
                || self.local_pawn == Some(network_id)
                || (self.bindings.mover_network_ids.contains_key(&network_id)
                    || self.bindings.region_network_ids.contains_key(&network_id))
                || self.bindings.is_presentation_bound(network_id)
            {
                continue;
            }
            self.update_remote_enemy_walk_playback_rate(
                registry,
                network_id,
                entity_id,
                0.0,
                frame_anim_time,
            );
        }
    }

    /// Before time sync can name a presentation tick, remote avatars hold their
    /// newest applied pose. Preserve that pose's pitch, clear motion-derived
    /// heading, and select client-local locomotion for a body at rest: the held
    /// pose shows no travel, though its newest posture and footing still pick
    /// crouch and jump states. With no presented tick to wait for, every queued host state is due
    /// and the newest applies, as it would have on receipt.
    pub(crate) fn apply_held_remote_player_presentation(
        &mut self,
        registry: &mut EntityRegistry,
        frame_anim_time: f64,
    ) {
        self.presented_player_inputs.aim_pitches.clear();
        self.presented_player_inputs.heading_yaws.clear();
        // Held poses show the newest applied snapshot; no per-remote tick.
        self.presented_state_ticks.clear();
        let mapped: Vec<(NetworkId, EntityId)> = self.map.iter().map(|(&n, &e)| (n, e)).collect();
        for (network_id, entity_id) in mapped {
            if !registry.exists(entity_id)
                || self.local_pawn == Some(network_id)
                || (self.bindings.mover_network_ids.contains_key(&network_id)
                    || self.bindings.region_network_ids.contains_key(&network_id))
                || self.bindings.is_presentation_bound(network_id)
                || !self.remote_player_locomotion.contains_key(&network_id)
            {
                continue;
            }
            if let Some(aim_pitch) = self.interp.newest_aim_pitch(network_id) {
                self.presented_player_inputs
                    .aim_pitches
                    .insert(network_id, aim_pitch);
            }
            let stance = self.interp.newest_stance(network_id);
            self.apply_due_remote_player_correction(registry, network_id, entity_id, None);
            self.update_remote_player_locomotion(
                registry,
                network_id,
                entity_id,
                Vec3::ZERO,
                stance,
                0.0,
                frame_anim_time,
                RemotePlayerPresentation::HeldNewest,
            );
        }
    }

    pub(crate) fn cache_remote_presentation_complete(
        &mut self,
        network_id: NetworkId,
        complete: bool,
    ) {
        if complete && self.map.contains_key(&network_id) {
            self.remote_presentation_complete.insert(network_id);
        } else {
            self.remote_presentation_complete.remove(&network_id);
        }
    }

    /// Record (or clear) the immutable descriptor data used to derive a remote
    /// enemy's walk playback rate. Called by descriptor-aware receive glue, while
    /// all per-frame registry writes remain in [`Self::sample_into_registry`].
    pub(crate) fn cache_remote_enemy_walk_playback(
        &mut self,
        network_id: NetworkId,
        reference: Option<(f32, String, Option<f32>)>,
    ) {
        match reference {
            Some((move_speed, walk_state, derived_travel_speed)) => {
                self.remote_enemy_walk_playback.insert(
                    network_id,
                    RemoteEnemyWalkPlayback {
                        move_speed,
                        walk_state,
                        derived_travel_speed,
                    },
                );
            }
            None => {
                self.remote_enemy_walk_playback.remove(&network_id);
            }
        }
    }

    /// Retain the descriptor identity already carried by the snapshot while
    /// descriptor-aware presentation materialization has it in scope. The cache keeps
    /// the class tag; readers decode it, so the representation (pawn, projectile or
    /// descriptor entity) stays available beside the canonical name.
    pub(crate) fn cache_remote_entity_class(&mut self, network_id: NetworkId, entity_class: &str) {
        self.remote_entity_classes
            .insert(network_id, entity_class.to_string());
    }

    /// Record immutable descriptor locomotion data after remote-player mesh
    /// materialization. `None` leaves an unusual/meshless descriptor transform-only
    /// and prevents it from being mistaken for an avatar by the pose-input pass.
    pub(crate) fn cache_remote_player_locomotion(
        &mut self,
        network_id: NetworkId,
        reference: Option<RemotePlayerLocomotionReference>,
    ) {
        match reference {
            // Re-materializing a mapped avatar (a weapon swap) refreshes the
            // descriptor data and keeps any correction it is honoring.
            Some(reference) => match self.remote_player_locomotion.get_mut(&network_id) {
                Some(locomotion) => locomotion.reference = reference,
                None => {
                    self.remote_player_locomotion
                        .insert(network_id, RemotePlayerLocomotion::new(reference));
                }
            },
            None => {
                self.remote_player_locomotion.remove(&network_id);
            }
        }
    }

    /// Seed a freshly materialized remote avatar's forward/backward memory from
    /// the host's state. Inside the dead zone the memory decides, so an unseeded
    /// joiner would first derive the opposite fold and flip once the next delta
    /// corrects it. Names are checked as declared: the host enters a backward
    /// state only when its clip resolves.
    pub(crate) fn seed_remote_player_direction(
        &self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        entity_id: EntityId,
        state: &str,
    ) {
        let Some(locomotion) = self.remote_player_locomotion.get(&network_id) else {
            return;
        };
        if let Some(direction) = locomotion.reference.states.direction_of(state, |_| true) {
            crate::sim::store_locomotion_direction(registry, entity_id, direction);
        }
    }

    /// Apply the newest queued host state for a remote player that the
    /// presentation has reached: every one at or before `presented_tick`, or all
    /// of them for `None`.
    fn apply_due_remote_player_correction(
        &mut self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        entity_id: EntityId,
        presented_tick: Option<f64>,
    ) {
        let due = self
            .remote_player_locomotion
            .get_mut(&network_id)
            .and_then(|locomotion| locomotion.take_due_correction(presented_tick));
        if let Some(state) = due
            && !self
                .apply_remote_player_animation_correction(registry, network_id, entity_id, &state)
        {
            log::trace!(
                "[Net] remote player {network_id:?} did not take mesh animation state `{state}`"
            );
        }
    }

    /// Apply host animation authority for a remote player. A disagreement with the
    /// client-local velocity prediction deliberately enters through the normal
    /// transition API, preserving the descriptor's short avatar crossfade instead
    /// of replacing the displayed pose with a hard snap.
    ///
    /// Jump one-shots are the exception: this client times them from its own
    /// presented takeoff and landing, and the host stamps its own with the frame's
    /// animation clock, so after a host hitch its `jump_up` or `jump_down` can
    /// outlast this client's. A host one-shot applies only while this client's
    /// phase still plays it, never re-entering one the client has left.
    fn apply_remote_player_animation_correction(
        &mut self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        entity_id: EntityId,
        authoritative_state: &str,
    ) -> bool {
        let Some(locomotion) = self.remote_player_locomotion.get(&network_id) else {
            return false;
        };
        let phase = registry
            .get_component::<MeshComponent>(entity_id)
            .ok()
            .and_then(|mesh| mesh.animation.as_ref())
            .map(|animation| animation.locomotion_jump)
            .unwrap_or_default();
        if !locomotion
            .reference
            .states
            .admits_state(authoritative_state, phase)
        {
            // The host has named another state, so a hold on an earlier one is
            // stale; released, this client's own derivation, already in the
            // matching phase, switches on the same frame.
            if let Some(locomotion) = self.remote_player_locomotion.get_mut(&network_id)
                && locomotion
                    .authoritative_correction_state
                    .as_deref()
                    .is_some_and(|held| held != authoritative_state)
            {
                locomotion.authoritative_correction_state = None;
            }
            return false;
        }
        let predicted_state = locomotion.client_derived_state.as_deref();
        let correcting = predicted_state.is_some_and(|predicted| predicted != authoritative_state);
        if correcting {
            log::trace!(
                "[Net] remote player {network_id:?} correcting locomotion {predicted_state:?} -> {authoritative_state}"
            );
        }
        let applied = apply_mesh_animation_state(registry, entity_id, authoritative_state, true);
        if applied
            && correcting
            && let Some(reference) = self.remote_player_locomotion.get_mut(&network_id)
        {
            reference.authoritative_correction_state = Some(authoritative_state.to_string());
        }
        applied
    }

    /// Apply the rate for one presented remote pose. Only an enemy whose current
    /// mesh state is the descriptor's alert-mapped walk state receives its displayed
    /// XZ speed; every other state deliberately rests at rate 1.
    fn update_remote_enemy_walk_playback_rate(
        &self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        entity_id: EntityId,
        speed_xz: f32,
        frame_anim_time: f64,
    ) {
        let Some(reference) = self.remote_enemy_walk_playback.get(&network_id) else {
            return;
        };
        let raw_ratio = registry
            .get_component::<MeshComponent>(entity_id)
            .ok()
            .and_then(|mesh| mesh.animation.as_ref())
            .and_then(|animation| {
                if animation.current_state != reference.walk_state || !animation.speed_scale {
                    return Some(1.0);
                }
                let state = animation.states.get(&animation.current_state)?;
                let effective = state.effective_travel_speed(reference.derived_travel_speed);
                Some(MeshAnimation::locomotion_rate_ratio(
                    speed_xz,
                    effective,
                    reference.move_speed,
                ))
            });
        let Some(raw_ratio) = raw_ratio else {
            return;
        };

        let needs_rebase = registry
            .get_component::<MeshComponent>(entity_id)
            .ok()
            .and_then(|mesh| mesh.animation.as_ref())
            .is_some_and(|animation| animation.playback_rate_needs_update(raw_ratio));
        if !needs_rebase {
            return;
        }

        // `sample_into_registry` intentionally allocates its mapped-id vector
        // before this helper to split the map and registry borrows. This helper
        // itself clones the mesh only when the animation predicate says a rebase
        // write is necessary.
        let Ok(mut mesh) = registry.get_component::<MeshComponent>(entity_id).cloned() else {
            return;
        };
        let Some(animation) = mesh.animation.as_mut() else {
            return;
        };
        animation.update_playback_rate(raw_ratio, frame_anim_time);
        let _ = registry.set_component(entity_id, mesh);
    }

    /// Derive one remote avatar's locomotion state and rate from the interpolated
    /// intrinsic player velocity, classified forward or backward against the
    /// presented aim, and the replicated posture and footing presented with it. The
    /// host's mesh state remains the correction authority; this is only the
    /// responsive between-snapshot path. The jump phase advances from the
    /// presented footing even while a correction is held, so the sequence memory
    /// follows the presented motion rather than the host state. Returns the
    /// direction the avatar's legs present this frame.
    #[allow(clippy::too_many_arguments)]
    fn update_remote_player_locomotion(
        &mut self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        entity_id: EntityId,
        velocity: Vec3,
        stance: crate::sim::LocomotionStance,
        aim_yaw: f32,
        frame_anim_time: f64,
        presentation: RemotePlayerPresentation,
    ) -> LocomotionDirection {
        let Some(locomotion) = self.remote_player_locomotion.get_mut(&network_id) else {
            return LocomotionDirection::Forward;
        };
        // A held newest pose is ahead of the first interpolated one, so the
        // footing would run backward across the handover: a held rise would read
        // its older grounded tick as a landing. That first interpolated pose
        // resyncs the phase from its own footing instead.
        let resync_jump_phase = match presentation {
            RemotePlayerPresentation::HeldNewest => {
                locomotion.resync_jump_phase = true;
                false
            }
            RemotePlayerPresentation::Interpolated => locomotion.resync_jump_phase,
        };
        let speed_xz = horizontal_speed(velocity);
        let Some(animation) = registry
            .get_component::<MeshComponent>(entity_id)
            .ok()
            .and_then(|mesh| mesh.animation.as_ref())
        else {
            return LocomotionDirection::Forward;
        };
        // Consumed only once a mesh animation can take the resync.
        if resync_jump_phase {
            locomotion.resync_jump_phase = false;
        }
        let reference = &locomotion.reference;
        let usable = |state: &str| animation.is_state_usable(state);
        let stored_direction = animation.locomotion_direction;
        let stored_jump = animation.locomotion_jump;
        let current_state = animation.current_state.as_str();
        // A host correction is honored until this client's own derivation agrees
        // with it rather than for its crossfade alone: releasing earlier reverts
        // to the disagreeing prediction, and the host's next resend corrects
        // again.
        let correction_held = locomotion
            .authoritative_correction_state
            .as_deref()
            .is_some_and(|authoritative| authoritative == current_state);
        // The held state seeds the classification rather than overriding it:
        // inside the dead zone the legs fold with the corrected clip, and outside
        // it travel still decides, keeping the twist bounded while the presented
        // motion catches up.
        let previous = if correction_held {
            reference
                .states
                .direction_of(current_state, usable)
                .unwrap_or(stored_direction)
        } else {
            stored_direction
        };
        let direction = crate::sim::classify_locomotion_direction(velocity, aim_yaw, previous);
        let mut input = crate::sim::LocomotionInput {
            speed_xz,
            walk_speed: reference.walk_speed,
            run_speed: reference.run_speed,
            direction,
            posture: stance.posture,
            footing: stance.footing,
            jump_phase: stored_jump,
            current_state,
        };
        input.jump_phase = if resync_jump_phase {
            reference.states.resync_jump_phase(stance.footing, usable)
        } else {
            reference
                .states
                .advance_jump_phase(&input, frame_anim_time, usable, |state| {
                    reference.clip_durations.get(state).copied()
                })
        };
        let jump = input.jump_phase;
        let (target_state, tier) = reference.states.select(&input, usable);
        let target_state = target_state.unwrap_or(&reference.idle_state);
        let at_target = current_state == target_state;
        let tier_speed = match tier {
            crate::sim::LocomotionTierKind::Idle
            | crate::sim::LocomotionTierKind::Slide
            | crate::sim::LocomotionTierKind::Jump => None,
            crate::sim::LocomotionTierKind::Walk => Some(reference.walk_speed),
            crate::sim::LocomotionTierKind::Run => Some(reference.run_speed),
            crate::sim::LocomotionTierKind::Crouch => Some(reference.crouch_speed),
        };
        let moving = tier_speed.is_some();
        let derived_travel_speed =
            tier_speed.and_then(|_| reference.derived_travel_speeds.get(target_state).copied());
        let fallback_speed = tier_speed.unwrap_or(1.0);

        if locomotion.client_derived_state.as_deref() != Some(target_state) {
            locomotion.client_derived_state = Some(target_state.to_string());
        }
        if !correction_held || at_target {
            locomotion.authoritative_correction_state = None;
        }
        // A held correction plays the host's state at its authored rate.
        let target_reached = at_target
            || (!correction_held
                && matches!(
                    switch_animation_state(registry, entity_id, target_state),
                    SwitchResult::Switched | SwitchResult::AlreadyInState
                ));
        if direction != stored_direction || jump != stored_jump {
            crate::sim::store_locomotion_memory(registry, entity_id, direction, Some(jump));
        }

        let raw_ratio = registry
            .get_component::<MeshComponent>(entity_id)
            .ok()
            .and_then(|mesh| mesh.animation.as_ref())
            .and_then(|animation| {
                if !moving || !target_reached || !animation.speed_scale {
                    return Some(1.0);
                }
                let state = animation.states.get(target_state)?;
                let effective = state.effective_travel_speed(derived_travel_speed);
                Some(MeshAnimation::locomotion_rate_ratio(
                    speed_xz.max(0.0),
                    effective,
                    fallback_speed,
                ))
            });
        let needs_rebase = raw_ratio.is_some_and(|raw_ratio| {
            registry
                .get_component::<MeshComponent>(entity_id)
                .ok()
                .and_then(|mesh| mesh.animation.as_ref())
                .is_some_and(|animation| animation.playback_rate_needs_update(raw_ratio))
        });
        if let Some(raw_ratio) = raw_ratio.filter(|_| needs_rebase)
            && let Ok(mut mesh) = registry.get_component::<MeshComponent>(entity_id).cloned()
            && let Some(animation) = mesh.animation.as_mut()
        {
            animation.update_playback_rate(raw_ratio, frame_anim_time);
            let _ = registry.set_component(entity_id, mesh);
        }
        direction
    }

    /// Maps each live client-local entity id back to its network identity for the
    /// pose-input pass. The map is copied at the App boundary so the presentation
    /// system remains a registry-only CPU path.
    pub fn entity_network_ids(&self) -> HashMap<EntityId, NetworkId> {
        self.reverse_map.clone()
    }

    pub(crate) fn presented_player_inputs(&self) -> &ClientPresentationInputs {
        &self.presented_player_inputs
    }

    /// Whether `network_id` is awaiting a baseline refresh (tests / diagnostics).
    #[cfg(test)]
    pub(crate) fn is_pending_repair(&self, network_id: NetworkId) -> bool {
        self.pending_repairs.contains_key(&network_id)
    }

    /// The presented pose source for a mapped entity at a render tick (tests).
    #[cfg(test)]
    pub(crate) fn presented_source(
        &self,
        network_id: NetworkId,
        render_server_tick: f64,
    ) -> Option<super::interpolation::PoseSource> {
        self.interp
            .presented_pose(network_id, render_server_tick)
            .map(|p| p.source)
    }

    /// The stored baseline id for a mapped entity, if any (tests / diagnostics).
    #[cfg(test)]
    pub(crate) fn stored_baseline(&self, network_id: NetworkId) -> Option<u32> {
        self.baselines.get(&network_id).copied()
    }

    /// Number of interpolation samples buffered for a mapped remote entity (tests).
    /// Zero once the entity's buffer has been forgotten (despawn) or before any
    /// Transform sample has been recorded. Used by the E10 enemy-replication harness
    /// to prove the despawn apply forgets the `RemoteInterpolationBuffer` state.
    #[cfg(test)]
    pub(crate) fn sample_count(&self, network_id: NetworkId) -> usize {
        self.interp.sample_count(network_id)
    }

    /// The newest interpolation sample's transform for a remote entity (tests).
    #[cfg(test)]
    pub(crate) fn newest_sample_transform(&self, network_id: NetworkId) -> Option<Transform> {
        self.interp.newest_transform(network_id)
    }

    #[cfg(test)]
    pub(crate) fn mover_history_sample_count(&self, mover_id: u32) -> usize {
        self.mover_history.sample_count(mover_id)
    }

    #[cfg(test)]
    pub(crate) fn mover_history(&self) -> &MoverHistoryBuffer {
        &self.mover_history
    }

    /// Mutable for reconcile, whose replay memo lives beside the samples.
    pub(crate) fn mover_history_mut(&mut self) -> &mut MoverHistoryBuffer {
        &mut self.mover_history
    }

    /// The host tick, in snapshot-stamp units, whose replicated state `entity`'s
    /// presented pose shows this frame: the render tick, except across a blend
    /// (its leading sample) or on the near side of a teleport cut (before the
    /// cut's tick). `None` for an entity not sampled this frame, and for every
    /// remote before time sync names a render tick.
    pub fn presented_state_tick(&self, entity: EntityId) -> Option<f64> {
        let network_id = self.network_id_for_entity(entity)?;
        self.presented_state_ticks.get(&network_id).copied()
    }

    /// Server tick carried by the latest accepted snapshot. Presentation clocks
    /// use this until time-sync has enough samples to provide a fractional estimate.
    pub fn latest_server_tick(&self) -> Option<u32> {
        self.latest_sequence.map(|_| self.acked_server_tick)
    }

    /// The latest accepted snapshot sequence (tests / diagnostics).
    #[cfg(test)]
    pub(crate) fn latest_sequence(&self) -> Option<u32> {
        self.latest_sequence
    }
}

/// The first `Transform` payload in a component list, converted to an engine
/// `Transform`, or `None` if the list carries no (finite) Transform. A finite check
/// runs here so a non-finite spawn pose does not seed an entity.
fn first_transform(components: &[ComponentPayload]) -> Option<Transform> {
    components.iter().find_map(|payload| match payload {
        ComponentPayload::Transform(wire) if payload_is_finite(payload) => {
            Some(wire_to_transform(wire))
        }
        _ => None,
    })
}

fn first_mover_state(components: &[ComponentPayload]) -> Option<WireKinematicMoverState> {
    components.iter().find_map(|payload| match payload {
        ComponentPayload::KinematicMoverState(wire) if payload_is_finite(payload) => Some(*wire),
        _ => None,
    })
}

fn unknown_ground_mover_id(
    registry: &EntityRegistry,
    components: &[ComponentPayload],
) -> Option<u32> {
    for payload in components {
        let ComponentPayload::PlayerMovementState(wire) = payload else {
            continue;
        };
        let WireGroundRef::Mover(mover_id) = wire.ground else {
            continue;
        };
        if bindings::find_loaded_mover_entity(registry, mover_id).is_none() {
            return Some(mover_id);
        }
    }
    None
}

fn first_mesh_animation_state(components: &[ComponentPayload]) -> Option<String> {
    components.iter().find_map(|payload| match payload {
        ComponentPayload::MeshAnimationState(wire) => Some(wire.current_state.clone()),
        _ => None,
    })
}

#[derive(Debug, Clone)]
struct MoverApplyPlan {
    mover_id: u32,
    phase: KinematicMoverComponent,
    transform: Transform,
    pose: MoverPose,
    history_samples: Vec<MoverHistorySample>,
}

#[allow(clippy::too_many_arguments)]
fn prepare_mover_apply(
    registry: &EntityRegistry,
    id: EntityId,
    network_id: NetworkId,
    bound_mover_id: Option<u32>,
    wire: WireKinematicMoverState,
    server_tick: u32,
    mover_target_tick: u32,
    anchor_transform: Transform,
    tick_dt: f32,
) -> Option<MoverApplyPlan> {
    let mut phase = bindings::validate_kinematic_mover_state_binding(
        registry,
        id,
        network_id,
        bound_mover_id,
        &wire,
    )?;
    seed_kinematic_mover_phase(&mut phase, &wire)?;

    let phase_pose = mover_pose_for_current_phase(anchor_transform, &phase, tick_dt);
    let mut server_pose = phase_pose;
    server_pose.transform = anchor_transform;
    server_pose.tick_delta = if tick_dt.is_finite() && tick_dt > 0.0 {
        server_pose.linear_velocity * tick_dt
    } else {
        Vec3::ZERO
    };
    let anchor_drift = (phase_pose.transform.position - anchor_transform.position).length();
    if anchor_drift > 0.01 {
        log::warn!(
            "[Net] mover {network_id:?}/{} Transform anchor differs from phase pose by {anchor_drift:.3} m",
            wire.mover_id
        );
    }
    let mut history_samples = vec![MoverHistorySample {
        server_tick,
        pose: server_pose,
        phase: phase.clone(),
    }];

    let mut predicted_phase = phase.clone();
    let mut predicted_transform = server_pose.transform;
    let mut predicted_pose = server_pose;
    let advance_ticks = ticks_between(server_tick, mover_target_tick);
    for _ in 0..advance_ticks {
        predicted_pose =
            advance_mover_phase_one_tick(&mut predicted_phase, &mut predicted_transform, tick_dt);
    }
    if advance_ticks > 0 {
        history_samples.push(MoverHistorySample {
            server_tick: server_tick.wrapping_add(advance_ticks),
            pose: predicted_pose,
            phase: predicted_phase.clone(),
        });
    }

    Some(MoverApplyPlan {
        mover_id: wire.mover_id,
        phase: predicted_phase,
        transform: predicted_pose.transform,
        pose: predicted_pose,
        history_samples,
    })
}

fn seed_kinematic_mover_phase(
    mover: &mut KinematicMoverComponent,
    wire: &WireKinematicMoverState,
) -> Option<()> {
    if !matches!(wire.direction, -1 | 1) {
        return None;
    }
    mover.segment_index = wire.segment_index;
    mover.direction_sign = wire.direction;
    mover.mode = match wire.mode {
        0 => KinematicMoverMode::Once,
        1 => KinematicMoverMode::PingPong,
        _ => return None,
    };
    mover.segment_elapsed_ms = wire.segment_elapsed_ms;
    mover.wait_remaining_ms = wire.wait_remaining_ms;
    mover.started = wire.started;
    mover.completed = wire.completed;
    mover.blocked = wire.blocked;
    mover.current_linear_velocity = Vec3::from_array(wire.velocity);
    mover.target_segment = wire.target_segment;
    mover.spin_angle_rad = wire.spin_angle_rad;
    mover.spin_angle_before_tick_rad = wire.spin_angle_before_tick_rad;
    mover.was_active_this_tick = wire.was_active_this_tick;
    mover.spin_rate_rad_s = wire.spin_rate_rad_s;
    mover.spin_target_rate_rad_s = wire.spin_target_rate_rad_s;
    Some(())
}

fn tick_le(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) <= 0
}

fn ticks_between(from: u32, to: u32) -> u32 {
    if tick_le(to, from) {
        0
    } else {
        to.wrapping_sub(from)
    }
}

pub(crate) fn apply_mesh_animation_state(
    registry: &mut EntityRegistry,
    id: EntityId,
    state: &str,
    allow_unresolved_initial: bool,
) -> bool {
    if matches!(
        switch_animation_state(registry, id, state),
        SwitchResult::Switched | SwitchResult::AlreadyInState
    ) {
        return true;
    }

    if !allow_unresolved_initial {
        return false;
    }

    let Ok(mut mesh) = registry.get_component::<MeshComponent>(id).cloned() else {
        return false;
    };
    let Some(animation) = mesh.animation.as_mut() else {
        return false;
    };
    if !animation.stage_unresolved_state(state) {
        return false;
    }
    registry.set_component(id, mesh).is_ok()
}

/// Encode an ack and any refresh requests into `ClientMessage` byte buffers ready for
/// `NetClient::send_input` on `Channel::Input`. The ack goes first (it carries the
/// sequence advance), then each refresh request. Kept here so the engine glue's
/// send path is a thin loop over already-encoded buffers.
pub(crate) fn encode_client_messages(outcome: &ApplyOutcome) -> Vec<Vec<u8>> {
    let mut buffers = Vec::new();
    if let Some(ack) = &outcome.ack {
        buffers.push(postretro_net::wire::encode(&ClientMessage::Ack(
            ack.clone(),
        )));
    }
    for req in &outcome.refresh_requests {
        buffers.push(postretro_net::wire::encode(
            &ClientMessage::BaselineRefresh(*req),
        ));
    }
    buffers
}

fn collect_presentation_payloads(
    outcome: &mut ApplyOutcome,
    nid: NetworkId,
    components: &[ComponentPayload],
) {
    outcome.presentation_payloads.extend(
        components
            .iter()
            .filter(|payload| is_presentation_payload(payload))
            .cloned()
            .map(|payload| (nid, payload)),
    );
}

#[cfg(test)]
mod tests {
    use super::super::interpolation::PoseSource;
    use super::*;
    use glam::Quat;
    use glam::Vec3;
    use postretro_entities::Transform;
    use postretro_entities::components::mesh::{
        RATE_MAX, RATE_MIN, resolve_pending_animation_stamps,
    };
    use postretro_net::wire::{
        WireGroundRef, WireKinematicMoverState, WireMeshAnimationState, WireMovementState,
        WirePlayerMovementState, WireTransform,
    };

    const EPSILON: f32 = 1e-6;

    fn transform_payload(x: f32) -> ComponentPayload {
        ComponentPayload::Transform(WireTransform {
            position: [x, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        })
    }

    fn transform_payload_at(position: [f32; 3], facing_yaw: f32) -> ComponentPayload {
        let rotation = Quat::from_rotation_y(facing_yaw);
        ComponentPayload::Transform(WireTransform {
            position,
            rotation: [rotation.x, rotation.y, rotation.z, rotation.w],
            scale: [1.0, 1.0, 1.0],
        })
    }

    pub(super) fn movement_payload() -> ComponentPayload {
        movement_payload_with_velocity([1.0, 0.0, 0.0])
    }

    fn movement_payload_with_velocity(velocity: [f32; 3]) -> ComponentPayload {
        movement_payload_with_velocity_and_pitch(velocity, 0.0)
    }

    fn movement_payload_with_velocity_and_pitch(
        velocity: [f32; 3],
        aim_pitch: f32,
    ) -> ComponentPayload {
        movement_payload_with(velocity, aim_pitch, WireMovementState::Normal)
    }

    fn movement_payload_with(
        velocity: [f32; 3],
        aim_pitch: f32,
        movement_state: WireMovementState,
    ) -> ComponentPayload {
        ComponentPayload::PlayerMovementState(WirePlayerMovementState {
            teleport: Default::default(),
            knockback_velocity: [0.0; 3],
            push_velocity: [0.0; 3],
            velocity,
            ground: WireGroundRef::World,
            air_jumps_remaining: 1,
            air_dashes_remaining: 1,
            dash_cooldown_ms: 0.0,
            air_ticks: 0,
            movement_state,
            coyote_timer_ms: 0.0,
            jump_buffer_timer_ms: 0.0,
            jump_spent: false,
            swim_reentry_latched: false,
            capsule_half_height: 0.8,
            capsule_eye_height: 1.5,
            aim_pitch,
        })
    }

    fn mesh_animation_payload(state: &str) -> ComponentPayload {
        ComponentPayload::MeshAnimationState(WireMeshAnimationState {
            current_state: state.to_string(),
        })
    }

    pub(super) fn mover_payload(mover_id: u32) -> ComponentPayload {
        ComponentPayload::KinematicMoverState(WireKinematicMoverState {
            mover_id,
            segment_index: 1,
            direction: -1,
            mode: 1,
            segment_elapsed_ms: 25.0,
            wait_remaining_ms: 5.0,
            started: true,
            completed: false,
            blocked: true,
            velocity: [0.5, 0.0, 0.0],
            target_segment: None,
            spin_angle_rad: 0.75,
            spin_angle_before_tick_rad: 0.5,
            was_active_this_tick: true,
            spin_rate_rad_s: 1.25,
            spin_target_rate_rad_s: 2.0,
        })
    }

    fn moving_mover_payload(mover_id: u32) -> ComponentPayload {
        ComponentPayload::KinematicMoverState(WireKinematicMoverState {
            mover_id,
            segment_index: 0,
            direction: 1,
            mode: 1,
            segment_elapsed_ms: 0.0,
            wait_remaining_ms: 0.0,
            started: true,
            completed: false,
            blocked: false,
            velocity: [1.0, 0.0, 0.0],
            target_segment: None,
            spin_angle_rad: 0.25,
            spin_angle_before_tick_rad: 0.0,
            was_active_this_tick: true,
            spin_rate_rad_s: 1.5,
            spin_target_rate_rad_s: 1.5,
        })
    }

    fn closed_starting_mover_payload(mover_id: u32) -> ComponentPayload {
        ComponentPayload::KinematicMoverState(WireKinematicMoverState {
            mover_id,
            segment_index: 0,
            direction: 1,
            mode: 0,
            segment_elapsed_ms: 0.0,
            wait_remaining_ms: 0.0,
            started: true,
            completed: false,
            blocked: false,
            velocity: [0.0, 0.0, 0.0],
            target_segment: None,
            spin_angle_rad: 0.0,
            spin_angle_before_tick_rad: 0.0,
            was_active_this_tick: false,
            spin_rate_rad_s: 0.0,
            spin_target_rate_rad_s: 0.0,
        })
    }

    pub(super) fn spawn_loaded_mover(registry: &mut EntityRegistry, mover_id: u32) -> EntityId {
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                KinematicMoverComponent::new(
                    mover_id,
                    postretro_entities::KinematicMoverConfig {
                        waypoints: vec![Vec3::ZERO, Vec3::X],
                        waypoint_names: vec!["start".to_string(), "finish".to_string()],
                        speed_mps: 1.0,
                        wait_ms: 0.0,
                        mode: KinematicMoverMode::PingPong,
                        started: true,
                        spin_axis: Vec3::Y,
                        initial_spin_rate_rad_s: 0.0,
                        spin_accel_rad_s2: 0.0,
                        carry_yaw: false,
                    },
                ),
            )
            .unwrap();
        id
    }

    fn unresolved_mesh() -> MeshComponent {
        use postretro_entities::components::mesh::{
            AnimationState, DEFAULT_CROSSFADE_MS, InterruptPolicy, MeshAnimation,
        };
        use std::collections::HashMap;

        let unresolved = |clip: &str| AnimationState {
            clip: clip.to_string(),
            looping: true,
            crossfade_ms: DEFAULT_CROSSFADE_MS,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            clip_index: None,
        };
        let mut states = HashMap::new();
        states.insert("idle".to_string(), unresolved("Idle"));
        states.insert("locomotion".to_string(), unresolved("Locomotion"));
        states.insert("attack".to_string(), unresolved("Attack"));
        MeshComponent::animated(
            "models/remote_enemy/scene.gltf".to_string(),
            MeshAnimation::new(states, "idle".to_string()),
        )
    }

    fn remote_player_mesh() -> MeshComponent {
        use postretro_entities::components::mesh::{AnimationState, InterruptPolicy};
        use std::collections::HashMap;

        let state = |clip: &str, travel_speed: Option<f32>, clip_index| AnimationState {
            clip: clip.to_string(),
            looping: true,
            crossfade_ms: 50.0,
            interrupt: InterruptPolicy::Smooth,
            travel_speed,
            clip_index: Some(clip_index),
        };
        let mut states = HashMap::new();
        states.insert("idle".to_string(), state("idle", None, 0));
        states.insert(
            "walk_forward".to_string(),
            state("walk_forward", Some(60.0), 1),
        );
        MeshComponent::animated(
            "models/remote_player/scene.gltf".to_string(),
            MeshAnimation::new(states, "idle".to_string()),
        )
    }

    fn full_baseline(
        network_id: u32,
        baseline_id: u32,
        components: Vec<ComponentPayload>,
    ) -> EntityRecord {
        EntityRecord::FullBaseline {
            network_id,
            baseline_id,
            // Generic non-local fixture: intentionally omits `local_player`/
            // `last_processed_client_tick` to exercise the non-local replication path.
            last_processed_client_tick: None,
            local_player: false,
            // Generic (non-local) baseline fixture: no descriptor class.
            entity_class: None,
            active_weapon_archetype: None,
            projectile_presentation: None,
            presentation_binding: None,
            components,
        }
    }

    fn delta(
        network_id: u32,
        baseline_ref: u32,
        new_baseline_id: u32,
        components: Vec<ComponentPayload>,
    ) -> EntityRecord {
        EntityRecord::Delta {
            network_id,
            baseline_ref,
            new_baseline_id,
            // Generic non-local fixture: see `full_baseline`.
            last_processed_client_tick: None,
            local_player: false,
            // Generic (non-local) delta fixture: no descriptor class.
            entity_class: None,
            active_weapon_archetype: None,
            projectile_presentation: None,
            presentation_binding: None,
            components,
        }
    }

    fn snapshot(sequence: u32, server_tick: u32, records: Vec<EntityRecord>) -> SnapshotMessage {
        SnapshotMessage {
            sequence,
            server_tick,
            records,
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
            faction_sentiment_record: None,
            ghost_audio: None,
            server_script_time: 0.0,
            level_tick: 0,
        }
    }

    fn entity_pos(registry: &EntityRegistry, id: EntityId) -> Vec3 {
        registry
            .get_component::<Transform>(id)
            .expect("entity has transform")
            .position
    }

    // --- Join-in-progress: full baseline spawns + maps, then deltas converge. ---
    #[test]
    fn teleport_armed_binding_updates_installed_region_and_demotion_preserves_it() {
        use postretro_entities::TeleporterComponent;
        use postretro_net::wire::WireTeleporterState;
        let mut registry = EntityRegistry::new();
        let installed = registry.spawn(Transform::default());
        registry
            .set_component(
                installed,
                TeleporterComponent {
                    index: 0,
                    source_entity: 10,
                    armed: true,
                    arrival_sound: "local/cue".into(),
                    on_teleport: "local.event".into(),
                },
            )
            .unwrap();
        let mut client = ClientReplication::new();
        let state = |armed| {
            ComponentPayload::TeleporterState(WireTeleporterState {
                index: 0,
                source_entity: 10,
                armed,
            })
        };
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                1,
                vec![full_baseline(
                    7,
                    1,
                    vec![transform_payload(100.0), state(false)],
                )],
            ),
        );
        assert_eq!(client.entity_for_network_id(NetworkId(7)), Some(installed));
        let local = registry
            .get_component::<TeleporterComponent>(installed)
            .unwrap();
        assert!(!local.armed);
        assert_eq!(local.arrival_sound, "local/cue");
        assert!(
            entity_pos(&registry, installed).length() < 1.0e-6,
            "arming never overwrites authored transform"
        );
        client.apply_snapshot(
            &mut registry,
            &snapshot(2, 2, vec![delta(7, 1, 2, vec![state(true)])]),
        );
        assert!(
            registry
                .get_component::<TeleporterComponent>(installed)
                .unwrap()
                .armed
        );
        client.despawn_all_mapped(&mut registry);
        client.reset_for_demotion();
        assert!(registry.exists(installed));
        assert!(client.entity_for_network_id(NetworkId(7)).is_none());
    }

    #[test]
    fn teleport_client_observes_accepted_generations_and_cues_before_delayed_render_arrives() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        let movement = |generation, last| {
            let mut movement = movement_payload();
            let ComponentPayload::PlayerMovementState(ref mut wire) = movement else {
                unreachable!()
            };
            wire.teleport =
                postretro_net::wire::WireTeleportState::new(generation, 0.0, last, &[]).unwrap();
            movement
        };
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                10,
                vec![full_baseline(
                    7,
                    1,
                    vec![transform_payload(0.0), movement(0, u16::MAX)],
                )],
            ),
        );
        assert!(client.take_observed_teleports().is_empty());
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                20,
                vec![delta(
                    7,
                    1,
                    2,
                    vec![transform_payload(100.0), movement(1, 0)],
                )],
            ),
        );
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                3,
                30,
                vec![full_baseline(
                    7,
                    3,
                    vec![transform_payload(200.0), movement(2, 1)],
                )],
            ),
        );
        client.sample_into_registry(&mut registry, 15.0, 0.0);
        let entity = client.entity_for_network_id(NetworkId(7)).unwrap();
        assert!(entity_pos(&registry, entity).length() < 1.0e-6);
        // Two accepted generation changes in one frame are two cues, in order,
        // each at its own accepted arrival point.
        let cues = client.take_observed_teleports();
        assert_eq!(cues.len(), 2);
        assert_eq!((cues[0].generation, cues[0].last_teleporter), (1, 0));
        assert!((cues[0].arrival.x - 100.0).abs() < 1.0e-6);
        assert_eq!((cues[1].generation, cues[1].last_teleporter), (2, 1));
        assert!((cues[1].arrival.x - 200.0).abs() < 1.0e-6);
        assert!(client.take_observed_teleports().is_empty());
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                20,
                vec![full_baseline(
                    7,
                    2,
                    vec![transform_payload(100.0), movement(1, 0)],
                )],
            ),
        );
        assert!(
            client.take_observed_teleports().is_empty(),
            "stale known-identity records cannot replay cues"
        );
    }

    #[test]
    fn full_baseline_spawns_and_delta_converges_with_stable_mapping() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(7, 1, vec![transform_payload(2.0)])],
            ),
        );
        // Spawned, mapped, baseline stored, acked, sequence advanced.
        let id = *client.map().get(&NetworkId(7)).expect("mapped");
        assert!(registry.exists(id));
        assert!((entity_pos(&registry, id).x - 2.0).abs() < EPSILON);
        assert_eq!(client.stored_baseline(NetworkId(7)), Some(1));
        assert_eq!(client.latest_sequence(), Some(0));
        let ack = out.ack.expect("accepted snapshot acks");
        assert_eq!(ack.latest_snapshot_sequence, 0);
        assert_eq!(ack.acked_server_tick, 100);
        assert_eq!(ack.entity_baselines, vec![(7, 1)]);
        assert!(out.refresh_requests.is_empty());

        // Delta from baseline 1 -> 2 moves the entity in place (no respawn).
        let out2 = client.apply_snapshot(
            &mut registry,
            &snapshot(1, 101, vec![delta(7, 1, 2, vec![transform_payload(9.0)])]),
        );
        let same = *client.map().get(&NetworkId(7)).expect("still mapped");
        assert_eq!(same, id, "delta mutates the same EntityId, no respawn");
        assert!((entity_pos(&registry, same).x - 9.0).abs() < EPSILON);
        assert_eq!(
            client.stored_baseline(NetworkId(7)),
            Some(2),
            "baseline advanced"
        );
        assert_eq!(out2.ack.unwrap().entity_baselines, vec![(7, 2)]);
    }

    #[test]
    fn mover_baseline_binds_loaded_mover_by_mover_id_without_spawning() {
        let mut registry = EntityRegistry::new();
        let mover_entity = spawn_loaded_mover(&mut registry, 42);
        let mut client = ClientReplication::new();

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![transform_payload(3.0), mover_payload(42)],
                )],
            ),
        );

        assert_eq!(client.map().get(&NetworkId(7)), Some(&mover_entity));
        assert_eq!(client.stored_baseline(NetworkId(7)), Some(1));
        assert_eq!(out.ack.unwrap().entity_baselines, vec![(7, 1)]);
        assert!(out.refresh_requests.is_empty());
        assert_eq!(
            client.sample_count(NetworkId(7)),
            0,
            "movers are not remote-interpolated"
        );
        assert_eq!(client.mover_history_sample_count(42), 1);
        let movers: Vec<_> = registry
            .iter_with_kind(ComponentKind::KinematicMover)
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            movers,
            vec![mover_entity],
            "baseline did not spawn a duplicate mover"
        );
        assert!((entity_pos(&registry, mover_entity).x - 3.0).abs() < EPSILON);
        let mover = registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap();
        assert_eq!(mover.segment_index, 1);
        assert_eq!(mover.direction_sign, -1);
        assert_eq!(mover.wait_remaining_ms, 5.0);
        assert!(
            mover.blocked,
            "reconciliation must apply the host stop hold"
        );
        assert!((mover.spin_angle_rad - 0.75).abs() < EPSILON);
        assert!((mover.spin_angle_before_tick_rad - 0.5).abs() < EPSILON);
        assert!(mover.was_active_this_tick);
        assert!((mover.spin_rate_rad_s - 1.25).abs() < EPSILON);
        assert!((mover.spin_target_rate_rad_s - 2.0).abs() < EPSILON);
    }

    #[test]
    fn client_predicted_mover_phase_matches_host_door_occlusion_phase() {
        let ComponentPayload::KinematicMoverState(closed_starting_phase) =
            closed_starting_mover_payload(42)
        else {
            unreachable!("fixture creates a kinematic mover payload");
        };

        let mut host_phase = KinematicMoverComponent::new(
            42,
            postretro_entities::KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::X],
                waypoint_names: vec!["closed".to_string(), "open".to_string()],
                speed_mps: 1.0,
                wait_ms: 0.0,
                mode: KinematicMoverMode::Once,
                started: true,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        seed_kinematic_mover_phase(&mut host_phase, &closed_starting_phase)
            .expect("host fixture phase is valid");
        assert!(
            crate::kinematic_mover::mover_is_docked_closed(&host_phase),
            "a start_on_spawn door is closed before its first simulation tick"
        );

        let mut registry = EntityRegistry::new();
        let mover_entity = spawn_loaded_mover(&mut registry, 42);
        let mut client = ClientReplication::new();
        client.apply_snapshot_with_mover_target_tick(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![transform_payload(0.0), closed_starting_mover_payload(42)],
                )],
            ),
            100,
            0.25,
        );
        let client_closed_phase = registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .expect("snapshot seeds the loaded mover");
        assert_eq!(
            crate::kinematic_mover::mover_is_docked_closed(client_closed_phase),
            crate::kinematic_mover::mover_is_docked_closed(&host_phase),
            "both peers derive occlusion from the same live component phase"
        );
        assert_eq!(
            client.sample_count(NetworkId(7)),
            0,
            "movers remain outside the interpolation sampler"
        );

        let mut host_transform = Transform::default();
        for _ in 0..2 {
            advance_mover_phase_one_tick(&mut host_phase, &mut host_transform, 0.25);
        }
        client.apply_snapshot_with_mover_target_tick(
            &mut registry,
            &snapshot(
                1,
                100,
                vec![delta(
                    7,
                    1,
                    2,
                    vec![transform_payload(0.0), closed_starting_mover_payload(42)],
                )],
            ),
            102,
            0.25,
        );
        let client_opening_phase = registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .expect("predicted phase remains in the live registry");
        assert_eq!(client_opening_phase.segment_index, host_phase.segment_index);
        assert!(
            (client_opening_phase.segment_elapsed_ms - host_phase.segment_elapsed_ms).abs()
                <= EPSILON
        );
        assert_eq!(
            crate::kinematic_mover::mover_is_docked_closed(client_opening_phase),
            crate::kinematic_mover::mover_is_docked_closed(&host_phase),
            "the predicted opening phase unblocks on both peers before render"
        );
        assert!(
            !crate::kinematic_mover::mover_is_docked_closed(client_opening_phase),
            "a locally predicted opening door cannot cull its now-visible interior"
        );
    }

    #[test]
    fn non_finite_mover_angular_payloads_do_not_reach_phase_seeding() {
        for (field, poison) in [
            ("spin_angle_rad", f32::NAN),
            ("spin_angle_before_tick_rad", f32::NAN),
            ("spin_rate_rad_s", f32::INFINITY),
            ("spin_target_rate_rad_s", f32::NEG_INFINITY),
        ] {
            let mut payload = mover_payload(42);
            let ComponentPayload::KinematicMoverState(wire) = &mut payload else {
                unreachable!("mover_payload returns a kinematic mover payload");
            };
            match field {
                "spin_angle_rad" => wire.spin_angle_rad = poison,
                "spin_angle_before_tick_rad" => wire.spin_angle_before_tick_rad = poison,
                "spin_rate_rad_s" => wire.spin_rate_rad_s = poison,
                "spin_target_rate_rad_s" => wire.spin_target_rate_rad_s = poison,
                _ => unreachable!("test covers every replicated angular field"),
            }

            assert!(!payload_is_finite(&payload), "{field} must be finite");
            assert!(
                first_mover_state(&[payload]).is_none(),
                "a non-finite {field} must be rejected before phase seeding"
            );
        }
    }

    #[test]
    fn mover_baseline_with_unknown_mover_id_does_not_spawn() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![transform_payload(3.0), mover_payload(99)],
                )],
            ),
        );

        assert!(client.map().get(&NetworkId(7)).is_none());
        assert!(out.ack.unwrap().entity_baselines.is_empty());
        assert!(
            out.ignored.iter().any(|ignored| matches!(
                ignored,
                IgnoredPayload::UnknownKinematicMover { mover_id: 99, .. }
            )),
            "unknown mover id is rejected engine-side"
        );
        assert_eq!(registry.iter_with_kind(ComponentKind::Transform).count(), 0);
    }

    #[test]
    fn movement_payload_with_unknown_ground_mover_is_not_baselined_or_acked() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        let mut movement = movement_payload_with_velocity([0.0, 0.0, 0.0]);
        if let ComponentPayload::PlayerMovementState(wire) = &mut movement {
            wire.ground = WireGroundRef::Mover(99);
        }

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(7, 1, vec![transform_payload(3.0), movement])],
            ),
        );

        assert!(client.map().get(&NetworkId(7)).is_none());
        assert_eq!(client.stored_baseline(NetworkId(7)), None);
        assert!(out.ack.unwrap().entity_baselines.is_empty());
        assert!(
            out.ignored.iter().any(|ignored| matches!(
                ignored,
                IgnoredPayload::UnknownGroundMover { mover_id: 99, .. }
            )),
            "unknown ground mover id is rejected engine-side"
        );
        assert_eq!(registry.iter_with_kind(ComponentKind::Transform).count(), 0);
    }

    // Regression: a mismatched mover payload must not overwrite the established
    // NetworkId -> mover_id binding or record history under the bad mover id.
    #[test]
    fn mismatched_mover_state_does_not_rebind_or_record_bad_mover_id() {
        let mut registry = EntityRegistry::new();
        let mover_entity = spawn_loaded_mover(&mut registry, 42);
        let mut client = ClientReplication::new();

        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![transform_payload(3.0), mover_payload(42)],
                )],
            ),
        );

        let before = entity_pos(&registry, mover_entity);
        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                101,
                vec![delta(
                    7,
                    1,
                    2,
                    vec![transform_payload(9.0), mover_payload(99)],
                )],
            ),
        );

        assert!(out.ack.unwrap().entity_baselines.is_empty());
        assert_eq!(client.stored_baseline(NetworkId(7)), Some(1));
        assert_eq!(
            client.bindings.mover_network_ids.get(&NetworkId(7)),
            Some(&42)
        );
        assert_eq!(client.mover_history_sample_count(99), 0);
        assert_eq!(client.mover_history_sample_count(42), 1);
        assert!(
            (entity_pos(&registry, mover_entity) - before).length() < EPSILON,
            "bad mover payload does not apply its Transform"
        );
    }

    // Regression: the mover Transform payload is the authoritative snapshot anchor;
    // the phase payload seeds prediction but must not replace the server pose sample.
    #[test]
    fn mover_snapshot_uses_transform_payload_as_server_tick_anchor() {
        let mut registry = EntityRegistry::new();
        let mover_entity = spawn_loaded_mover(&mut registry, 42);
        let mut client = ClientReplication::new();

        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![transform_payload(3.0), mover_payload(42)],
                )],
            ),
        );

        assert!((entity_pos(&registry, mover_entity).x - 3.0).abs() < EPSILON);
        let anchored = client
            .mover_history()
            .pose_at_tick(42, 100, DEFAULT_MOVER_TICK_DT)
            .expect("server tick mover sample");
        assert!((anchored.transform.position.x - 3.0).abs() < EPSILON);
    }

    // Regression: a received mover phase is fast-forwarded to the caller's target
    // tick before becoming live, and the fast-forwarded history sample carries the
    // mover tick delta replay needs for carry.
    #[test]
    fn mover_snapshot_fast_forwards_and_records_nonzero_advanced_tick_delta() {
        let mut registry = EntityRegistry::new();
        let mover_entity = spawn_loaded_mover(&mut registry, 42);
        let mut client = ClientReplication::new();

        client.apply_snapshot_with_mover_target_tick(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![transform_payload(0.0), moving_mover_payload(42)],
                )],
            ),
            102,
            0.25,
        );

        assert!((entity_pos(&registry, mover_entity).x - 0.5).abs() < EPSILON);
        assert_eq!(client.mover_history_sample_count(42), 2);
        let advanced = client
            .mover_history()
            .pose_at_tick(42, 102, 0.25)
            .expect("advanced mover history sample");
        assert!(
            (advanced.tick_delta - Vec3::new(0.25, 0.0, 0.0)).length() < EPSILON,
            "advanced mover sample stores replay carry delta"
        );
        assert!((advanced.transform.position.x - 0.5).abs() < EPSILON);
        assert!((advanced.angular_velocity - Vec3::Y * 1.5).length() < EPSILON);
        assert!(
            advanced
                .tick_rotation_delta
                .abs_diff_eq(Quat::from_rotation_y(0.375), EPSILON),
            "fast-forward replay reports the rotation its phase advance actually applied"
        );
        assert!(
            advanced
                .transform
                .rotation
                .abs_diff_eq(Quat::from_rotation_y(1.0), EPSILON),
            "fast-forwarded mover orientation derives from the replicated phase"
        );
    }

    // Regression: once-mode completion erased the angular kinematics applied
    // during its final predicted tick from mover history.
    #[test]
    fn mover_completion_history_preserves_final_tick_rotation_then_replays_stopped() {
        let mut registry = EntityRegistry::new();
        let mover_entity = spawn_loaded_mover(&mut registry, 42);
        let mut client = ClientReplication::new();
        let mut mover = moving_mover_payload(42);
        let ComponentPayload::KinematicMoverState(wire) = &mut mover else {
            unreachable!("moving_mover_payload returns a mover payload");
        };
        wire.mode = 0;
        wire.spin_angle_rad = 0.0;
        wire.spin_rate_rad_s = 1.0;
        wire.spin_target_rate_rad_s = 1.0;

        client.apply_snapshot_with_mover_target_tick(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(7, 1, vec![transform_payload(0.0), mover])],
            ),
            101,
            1.0,
        );

        let completion = client
            .mover_history()
            .pose_at_tick(42, 101, 1.0)
            .expect("completion tick history sample");
        let phase = registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .expect("live mover phase after completion");
        let transform = *registry
            .get_component::<Transform>(mover_entity)
            .expect("live mover transform after completion");
        let reconstructed = mover_pose_for_current_phase(transform, phase, 1.0);
        let replayed = client
            .mover_history()
            .pose_at_tick(42, 102, 1.0)
            .expect("replay from completed phase");

        assert!(phase.completed);
        assert!(
            completion
                .transform
                .rotation
                .abs_diff_eq(Quat::from_rotation_y(1.0), EPSILON)
        );
        assert!((completion.angular_velocity - Vec3::Y).length() < EPSILON);
        assert!(
            completion
                .tick_rotation_delta
                .abs_diff_eq(Quat::from_rotation_y(1.0), EPSILON)
        );
        assert!((reconstructed.angular_velocity - completion.angular_velocity).length() < EPSILON);
        assert!(
            reconstructed
                .tick_rotation_delta
                .abs_diff_eq(completion.tick_rotation_delta, EPSILON),
            "replicated completion phase must reconstruct the actual completion tick"
        );
        assert_eq!(replayed.angular_velocity, Vec3::ZERO);
        assert_eq!(replayed.tick_rotation_delta, Quat::IDENTITY);
        assert!(
            replayed
                .transform
                .rotation
                .abs_diff_eq(Quat::from_rotation_y(1.0), EPSILON)
        );
    }

    // --- Delta with no held baseline: not applied, pending repair set, refresh
    // requested, unrelated state untouched. ---
    #[test]
    fn delta_without_held_baseline_requests_refresh_and_leaves_state() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        // Spawn entity 7 at baseline 1.
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                1,
                vec![full_baseline(7, 1, vec![transform_payload(0.0)])],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).unwrap();

        // A delta for entity 8, which the client never mapped: unappliable however
        // new its baseline id is.
        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(1, 2, vec![delta(8, 5, 6, vec![transform_payload(99.0)])]),
        );
        // State untouched: entity 8 not spawned, entity 7 unchanged.
        assert!(!client.map().contains_key(&NetworkId(8)));
        assert_eq!(client.stored_baseline(NetworkId(8)), None);
        assert!((entity_pos(&registry, id).x - 0.0).abs() < EPSILON);
        assert_eq!(client.stored_baseline(NetworkId(7)), Some(1));
        // Pending repair + a refresh request emitted, not acked.
        assert!(client.is_pending_repair(NetworkId(8)));
        assert!(!client.is_pending_repair(NetworkId(7)));
        assert_eq!(out.refresh_requests.len(), 1);
        let req = out.refresh_requests[0];
        assert_eq!(req.network_id, 8);
        assert_eq!(req.missing_baseline_ref, 5);
        // The ack carries the sequence advance but NO baseline for the unappliable
        // entity.
        let ack = out.ack.unwrap();
        assert!(
            ack.entity_baselines.is_empty(),
            "unknown-baseline delta not acked"
        );
    }

    // --- An empty delta newer than the held baseline is a valid no-op apply that
    // advances the baseline; resent at the held id it acks nothing. ---
    #[test]
    fn empty_delta_is_noop_apply_when_baseline_known() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                1,
                vec![full_baseline(7, 1, vec![transform_payload(3.0)])],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).unwrap();

        // Empty-component delta from the held baseline 1 -> 2: a valid no-op apply
        // that still advances the stored baseline and acks.
        let out =
            client.apply_snapshot(&mut registry, &snapshot(1, 2, vec![delta(7, 1, 2, vec![])]));
        assert!(
            (entity_pos(&registry, id).x - 3.0).abs() < EPSILON,
            "position unchanged"
        );
        assert_eq!(
            client.stored_baseline(NetworkId(7)),
            Some(2),
            "baseline advanced"
        );
        assert!(!client.is_pending_repair(NetworkId(7)));
        assert_eq!(out.ack.unwrap().entity_baselines, vec![(7, 2)]);

        // A repeated empty delta for the baseline already held is a metadata-only
        // resend: no refresh, no ack.
        let out2 =
            client.apply_snapshot(&mut registry, &snapshot(2, 3, vec![delta(7, 1, 2, vec![])]));
        assert!(!client.is_pending_repair(NetworkId(7)));
        assert!(out2.refresh_requests.is_empty());
        assert!(out2.ack.unwrap().entity_baselines.is_empty());
        assert_eq!(client.stored_baseline(NetworkId(7)), Some(2));
    }

    // --- Newest wins: a delta carries the full component set, so the host naming
    // its last *acked* baseline (while newer client acks are in flight) must not
    // starve the client. ---
    #[test]
    fn deltas_from_one_acked_ref_apply_in_order_without_refresh() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                1,
                vec![full_baseline(7, 1, vec![transform_payload(0.0)])],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).unwrap();

        // The host has only seen the ack for baseline 1, so both deltas name it.
        let out_b = client.apply_snapshot(
            &mut registry,
            &snapshot(1, 2, vec![delta(7, 1, 2, vec![transform_payload(2.0)])]),
        );
        let out_c = client.apply_snapshot(
            &mut registry,
            &snapshot(2, 3, vec![delta(7, 1, 3, vec![transform_payload(3.0)])]),
        );
        assert!((entity_pos(&registry, id).x - 3.0).abs() < EPSILON);
        assert_eq!(client.stored_baseline(NetworkId(7)), Some(3));
        assert!(out_b.refresh_requests.is_empty());
        assert!(out_c.refresh_requests.is_empty());
        assert!(!client.is_pending_repair(NetworkId(7)));
        assert_eq!(out_b.ack.unwrap().entity_baselines, vec![(7, 2)]);
        assert_eq!(out_c.ack.unwrap().entity_baselines, vec![(7, 3)]);
    }

    // Reordering and duplication inside the accepted-newest stream: a late
    // snapshot is dropped by the sequence gate and a duplicate id is skipped, but a
    // regressed id inside a newer snapshot breaks newest-wins and requests repair.
    #[test]
    fn duplicate_delta_skips_and_regressed_delta_requests_refresh() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                1,
                vec![full_baseline(7, 1, vec![transform_payload(0.0)])],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).unwrap();

        // C (baseline 3) arrives first and applies.
        let out_c = client.apply_snapshot(
            &mut registry,
            &snapshot(2, 3, vec![delta(7, 1, 3, vec![transform_payload(3.0)])]),
        );
        assert_eq!(out_c.ack.unwrap().entity_baselines, vec![(7, 3)]);

        // B (baseline 2) in its older snapshot is dropped by the sequence gate.
        let late_b = client.apply_snapshot(
            &mut registry,
            &snapshot(1, 2, vec![delta(7, 1, 2, vec![transform_payload(2.0)])]),
        );
        // A duplicate C, resent before the host saw the ack for 3.
        let dup_c = client.apply_snapshot(
            &mut registry,
            &snapshot(3, 4, vec![delta(7, 1, 3, vec![transform_payload(30.0)])]),
        );
        for out in [&late_b, &dup_c] {
            assert!(out.refresh_requests.is_empty());
            assert!(
                out.ack
                    .as_ref()
                    .is_none_or(|ack| ack.entity_baselines.is_empty()),
                "a skipped delta is not acked"
            );
        }
        assert!(!client.is_pending_repair(NetworkId(7)));

        // B's baseline in a newer snapshot is older than the held one: repair.
        let regressed_b = client.apply_snapshot(
            &mut registry,
            &snapshot(4, 5, vec![delta(7, 1, 2, vec![transform_payload(2.0)])]),
        );
        assert_eq!(regressed_b.refresh_requests.len(), 1);
        assert_eq!(regressed_b.refresh_requests[0].network_id, 7);
        assert_eq!(
            regressed_b.refresh_requests[0].reason,
            REFRESH_REASON_UNKNOWN_BASELINE
        );
        assert!(client.is_pending_repair(NetworkId(7)));
        assert!(
            regressed_b.ack.unwrap().entity_baselines.is_empty(),
            "a regressed delta is not acked"
        );

        // No state moved: the held baseline and pose are C's.
        assert!((entity_pos(&registry, id).x - 3.0).abs() < EPSILON);
        assert_eq!(client.stored_baseline(NetworkId(7)), Some(3));
    }

    #[test]
    fn baseline_newness_wraps_like_the_host_allocators() {
        assert!(baseline_is_newer(2, 1));
        assert!(!baseline_is_newer(1, 1));
        assert!(!baseline_is_newer(1, 2));
        assert!(baseline_is_newer(1, u32::MAX));
        assert!(!baseline_is_newer(u32::MAX, 1));
    }

    // Known identities never roll back; duplicate snapshots remain ignored.
    #[test]
    fn old_known_identity_and_duplicate_sequences_are_ignored() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                5,
                50,
                vec![full_baseline(7, 1, vec![transform_payload(1.0)])],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).unwrap();

        // An older baseline cannot change an already-known identity.
        let before = registry.exists(id);
        let mut count_before = client.map().len();
        let out_old = client.apply_snapshot(
            &mut registry,
            &snapshot(
                3,
                30,
                vec![full_baseline(7, 2, vec![transform_payload(7.0)])],
            ),
        );
        assert!(out_old.ack.is_none(), "ignored snapshot produces no ack");
        assert_eq!(client.stored_baseline(NetworkId(7)), Some(1));
        assert!((entity_pos(&registry, id).x - 1.0).abs() < 1.0e-6);
        assert_eq!(client.map().len(), count_before);
        assert_eq!(registry.exists(id), before);
        assert_eq!(
            client.latest_sequence(),
            Some(5),
            "latest sequence unchanged"
        );

        // A DUPLICATE of the latest sequence (5 == 5): also ignored.
        count_before = client.map().len();
        let out_dup = client.apply_snapshot(
            &mut registry,
            &snapshot(
                5,
                50,
                vec![full_baseline(9, 3, vec![transform_payload(8.0)])],
            ),
        );
        assert!(out_dup.ack.is_none());
        assert!(!client.map().contains_key(&NetworkId(9)));
        assert_eq!(client.map().len(), count_before);
    }

    // --- Mapped full baseline with a stale entity: drops the mapping, requests a
    // refresh, leaves unrelated entities untouched, does not ack. ---
    #[test]
    fn full_baseline_on_stale_mapping_requests_refresh_and_preserves_others() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        // Two entities mapped.
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                1,
                vec![
                    full_baseline(7, 1, vec![transform_payload(1.0)]),
                    full_baseline(8, 2, vec![transform_payload(2.0)]),
                ],
            ),
        );
        let id7 = *client.map().get(&NetworkId(7)).unwrap();
        let id8 = *client.map().get(&NetworkId(8)).unwrap();
        // Forcibly despawn entity 7 behind the client's back: the mapping is now stale.
        registry.despawn(id7).expect("live");

        // A full baseline for the stale-mapped 7 must drop the mapping + request a
        // refresh, while entity 8 is untouched.
        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                2,
                vec![full_baseline(7, 5, vec![transform_payload(3.0)])],
            ),
        );
        assert!(
            !client.map().contains_key(&NetworkId(7)),
            "stale mapping dropped"
        );
        assert!(
            client.stored_baseline(NetworkId(7)).is_none(),
            "stale baseline dropped"
        );
        assert!(client.is_pending_repair(NetworkId(7)));
        assert_eq!(out.refresh_requests.len(), 1);
        assert_eq!(out.refresh_requests[0].reason, REFRESH_REASON_STALE_MAPPING);
        assert!(
            out.ack.unwrap().entity_baselines.is_empty(),
            "stale baseline not acked"
        );
        // Entity 8 untouched.
        assert!(registry.exists(id8));
        assert_eq!(client.stored_baseline(NetworkId(8)), Some(2));
        assert!((entity_pos(&registry, id8).x - 2.0).abs() < EPSILON);
    }

    // Regression: stale-map repair removed the IDs but retained interpolation,
    // locomotion, and weapon state for the NetworkId reused by the repair spawn.
    #[test]
    fn stale_mapping_repair_respawn_reuses_network_id_with_clean_presentation_state() {
        let baseline = |baseline_id, x, aim_pitch| EntityRecord::FullBaseline {
            network_id: 7,
            baseline_id,
            last_processed_client_tick: None,
            local_player: false,
            entity_class: Some("player".to_string()),
            active_weapon_archetype: Some("reference_pistol".to_string()),
            projectile_presentation: None,
            presentation_binding: None,
            components: vec![
                transform_payload(x),
                movement_payload_with_velocity_and_pitch([12.0, 0.0, 4.0], aim_pitch),
            ],
        };
        let locomotion = || RemotePlayerLocomotionReference::idle_and_walk_forward(60.0);

        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(0, 100, vec![baseline(1, 1.0, 0.25)]),
        );
        let stale_id = *client.map().get(&NetworkId(7)).expect("initial mapping");
        client.cache_remote_player_locomotion(NetworkId(7), Some(locomotion()));
        client.sample_into_registry(&mut registry, 100.0, 0.0);
        assert_eq!(client.sample_count(NetworkId(7)), 1);
        assert!(client.remote_player_locomotion.contains_key(&NetworkId(7)));
        assert!(client.active_weapon_archetypes.contains_key(&NetworkId(7)));
        assert!(
            client
                .presented_player_inputs
                .aim_pitches
                .contains_key(&NetworkId(7))
        );

        registry.despawn(stale_id).expect("mapped entity was live");
        let unrelated = registry.spawn(Transform::default());
        let stale = client.apply_snapshot(
            &mut registry,
            &snapshot(1, 110, vec![baseline(5, 2.0, 0.5)]),
        );

        assert!(stale.ack.unwrap().entity_baselines.is_empty());
        assert!(client.is_pending_repair(NetworkId(7)));
        assert_eq!(client.sample_count(NetworkId(7)), 0);
        assert!(!client.remote_player_locomotion.contains_key(&NetworkId(7)));
        assert!(!client.active_weapon_archetypes.contains_key(&NetworkId(7)));
        assert!(
            !client
                .presented_player_inputs
                .aim_pitches
                .contains_key(&NetworkId(7))
        );

        let repaired = client.apply_snapshot(
            &mut registry,
            &snapshot(2, 120, vec![baseline(5, 3.0, 0.75)]),
        );
        let repaired_id = *client.map().get(&NetworkId(7)).expect("repair remapped");

        assert_ne!(
            repaired_id, stale_id,
            "repair uses a live-generation EntityId"
        );
        assert!(
            registry.exists(unrelated),
            "repair preserves the reused slot owner"
        );
        assert_eq!(client.sample_count(NetworkId(7)), 1);
        assert!(!client.remote_player_locomotion.contains_key(&NetworkId(7)));
        assert_eq!(
            client
                .active_weapon_archetypes
                .get(&NetworkId(7))
                .and_then(|archetype| archetype.as_deref()),
            Some("reference_pistol")
        );
        assert!(
            repaired.remote_entities.iter().any(|remote| {
                remote.network_id == NetworkId(7)
                    && remote.weapon_attachment_changed
                    && remote.active_weapon_archetype.as_deref() == Some("reference_pistol")
            }),
            "unchanged weapon identity is re-applied to the repaired presentation"
        );
    }

    // --- A refresh response (FullBaseline) clears the pending repair and re-maps. ---
    #[test]
    fn full_baseline_refresh_response_clears_pending_and_remaps() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        // A delta for an identity whose first baseline was lost enters the pending
        // set.
        client.apply_snapshot(
            &mut registry,
            &snapshot(1, 2, vec![delta(7, 99, 100, vec![transform_payload(5.0)])]),
        );
        assert!(client.is_pending_repair(NetworkId(7)));
        assert!(!client.map().contains_key(&NetworkId(7)));

        // The refresh response arrives as a FullBaseline: spawns and maps, clears
        // pending, acks.
        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                3,
                vec![full_baseline(7, 100, vec![transform_payload(5.0)])],
            ),
        );
        assert!(
            !client.is_pending_repair(NetworkId(7)),
            "refresh cleared pending"
        );
        assert!(client.map().contains_key(&NetworkId(7)));
        assert_eq!(client.stored_baseline(NetworkId(7)), Some(100));
        assert_eq!(out.ack.unwrap().entity_baselines, vec![(7, 100)]);
    }

    // --- Despawn: idempotent, drops mapping, acks tombstone; unknown despawn no-ops. ---
    // Display copies carry no provenance: client-side ghost eligibility tells a
    // remote pawn from a descriptor entity by the class tag its record carried.
    #[test]
    fn remote_entities_by_class_keeps_the_replicated_representation() {
        use super::super::descriptor_class::ReplicatedDescriptorClass;
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![
                    full_baseline(7, 1, vec![transform_payload_at([0.0; 3], 0.0)]),
                    full_baseline(8, 1, vec![transform_payload_at([1.0, 0.0, 0.0], 0.0)]),
                ],
            ),
        );
        client.cache_remote_entity_class(NetworkId(7), "pawn:player");
        client.cache_remote_entity_class(NetworkId(8), "descriptor:imp");
        let pawn = client.entity_for_network_id(NetworkId(7)).unwrap();
        let imp = client.entity_for_network_id(NetworkId(8)).unwrap();
        let mut classes: Vec<_> = client.remote_entities_by_class().collect();
        classes.sort_by_key(|(id, _)| *id);
        let mut expected = vec![
            (pawn, ReplicatedDescriptorClass::Pawn("player")),
            (imp, ReplicatedDescriptorClass::Descriptor("imp")),
        ];
        expected.sort_by_key(|(id, _)| *id);
        assert_eq!(classes, expected);
        assert_eq!(client.remote_entity_class(NetworkId(7)), Some("player"));
        assert_eq!(client.remote_entity_class(NetworkId(8)), Some("imp"));
    }

    #[test]
    fn despawn_drops_mapping_and_is_idempotent() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                1,
                vec![full_baseline(7, 1, vec![transform_payload(0.0)])],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).unwrap();

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                2,
                vec![EntityRecord::Despawn {
                    network_id: 7,
                    tombstone_id: 4,
                    reason: 0,
                }],
            ),
        );
        assert!(!registry.exists(id), "entity despawned");
        assert!(!client.map().contains_key(&NetworkId(7)), "mapping dropped");
        assert!(client.stored_baseline(NetworkId(7)).is_none());
        assert_eq!(out.ack.unwrap().despawn_tombstones, vec![(7, 4)]);

        // A despawn for an unknown / already-gone NetworkId is a no-op (still acks the
        // tombstone so the server stops resending).
        let out2 = client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                3,
                vec![EntityRecord::Despawn {
                    network_id: 7,
                    tombstone_id: 4,
                    reason: 0,
                }],
            ),
        );
        assert_eq!(out2.ack.unwrap().despawn_tombstones, vec![(7, 4)]);
    }

    #[test]
    fn reverse_entity_network_map_tracks_spawn_and_despawn() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                1,
                vec![full_baseline(7, 1, vec![transform_payload(0.0)])],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).unwrap();
        assert_eq!(
            client.network_id_for_entity(id),
            Some(NetworkId(7)),
            "client can name a mapped target on the wire"
        );
        assert_eq!(
            client.entity_for_network_id(NetworkId(7)),
            Some(id),
            "forward accessor resolves the mapped entity"
        );

        client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                2,
                vec![EntityRecord::Despawn {
                    network_id: 7,
                    tombstone_id: 4,
                    reason: 0,
                }],
            ),
        );
        assert_eq!(client.network_id_for_entity(id), None);
        assert_eq!(client.entity_for_network_id(NetworkId(7)), None);
    }

    // --- Unmapped full baseline WITHOUT a Transform does not spawn. ---
    #[test]
    fn unmapped_full_baseline_without_transform_does_not_spawn() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        // A baseline carrying only a movement payload (no Transform): invalid spawn.
        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(0, 1, vec![full_baseline(7, 1, vec![movement_payload()])]),
        );
        assert!(
            client.map().is_empty(),
            "no Transform -> no spawn, no mapping"
        );
        assert!(client.stored_baseline(NetworkId(7)).is_none());
        // Not acked (nothing applied), but the snapshot was accepted (sequence
        // advanced) so the ack still carries the sequence with no baselines.
        assert_eq!(client.latest_sequence(), Some(0));
        assert!(out.ack.unwrap().entity_baselines.is_empty());
    }

    // --- Movement payload on an unmapped full baseline with a Transform: Transform
    // applied, movement ignored with a typed diagnostic. ---
    #[test]
    fn movement_payload_without_local_component_is_ignored_with_diagnostic() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                1,
                vec![full_baseline(
                    7,
                    1,
                    vec![transform_payload(4.0), movement_payload()],
                )],
            ),
        );
        // Spawned from the Transform; the movement payload did NOT create a movement
        // component (the dumb mover is Transform-only).
        let id = *client
            .map()
            .get(&NetworkId(7))
            .expect("spawned from Transform");
        assert!((entity_pos(&registry, id).x - 4.0).abs() < EPSILON);
        assert!(
            !registry
                .has_component_kind(id, ComponentKind::PlayerMovement)
                .unwrap(),
            "wire movement subset must not construct a movement component"
        );
        assert_eq!(
            out.ignored,
            vec![IgnoredPayload::MovementWithoutLocalComponent { network_id: 7 }]
        );
        // The full baseline still applied + acked (the Transform did).
        assert_eq!(out.ack.unwrap().entity_baselines, vec![(7, 1)]);
    }

    // --- Non-finite transform in a full baseline does not spawn. ---
    #[test]
    fn full_baseline_with_non_finite_transform_does_not_spawn() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        let poisoned = ComponentPayload::Transform(WireTransform {
            position: [f32::NAN, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        });
        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(0, 1, vec![full_baseline(7, 1, vec![poisoned])]),
        );
        assert!(client.map().is_empty(), "non-finite spawn pose -> no spawn");
        assert!(out.ack.unwrap().entity_baselines.is_empty());
    }

    // --- Pending-repair cadence: resends at 5 Hz (every 200 ms), one per pending
    // entity, until cleared. ---
    #[test]
    fn pending_repair_resends_at_5hz_until_cleared() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        // Enter pending via a delta for an identity with no held baseline.
        client.apply_snapshot(
            &mut registry,
            &snapshot(1, 2, vec![delta(7, 99, 100, vec![transform_payload(1.0)])]),
        );
        assert!(client.is_pending_repair(NetworkId(7)));

        // Under the interval: nothing due.
        assert!(
            client.tick_pending_repairs(100.0).is_empty(),
            "no resend before 200ms"
        );
        // Crossing the interval (total 200ms): one resend.
        let due = client.tick_pending_repairs(100.0);
        assert_eq!(due.len(), 1, "one resend at the 200ms boundary");
        assert_eq!(due[0].network_id, 7);
        assert_eq!(due[0].missing_baseline_ref, 99);
        // Immediately after, the cadence resets: nothing due.
        assert!(client.tick_pending_repairs(50.0).is_empty());

        // The refresh response clears pending -> no further resends.
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                3,
                vec![full_baseline(7, 100, vec![transform_payload(1.0)])],
            ),
        );
        assert!(!client.is_pending_repair(NetworkId(7)));
        assert!(
            client.tick_pending_repairs(500.0).is_empty(),
            "cleared repair never resends"
        );
    }

    // --- A full baseline spawns; a later delta only touches its entity, leaving an
    // unrelated mapped entity's registry state and baseline intact. ---
    #[test]
    fn delta_apply_does_not_disturb_unrelated_entities() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                1,
                vec![
                    full_baseline(7, 1, vec![transform_payload(1.0)]),
                    full_baseline(8, 2, vec![transform_payload(2.0)]),
                ],
            ),
        );
        let id7 = *client.map().get(&NetworkId(7)).unwrap();
        let id8 = *client.map().get(&NetworkId(8)).unwrap();

        client.apply_snapshot(
            &mut registry,
            &snapshot(1, 2, vec![delta(7, 1, 3, vec![transform_payload(50.0)])]),
        );
        // Entity 7 moved + advanced; entity 8 untouched.
        assert!((entity_pos(&registry, id7).x - 50.0).abs() < EPSILON);
        assert_eq!(client.stored_baseline(NetworkId(7)), Some(3));
        assert!((entity_pos(&registry, id8).x - 2.0).abs() < EPSILON);
        assert_eq!(client.stored_baseline(NetworkId(8)), Some(2));
    }

    // --- Ack-production rule: a snapshot mixing an applied full baseline, an
    // unappliable delta, and a despawn acks ONLY the applied baseline + tombstone. ---
    #[test]
    fn ack_carries_only_applied_records() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        // Pre-map entity 9 so its despawn applies.
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                1,
                vec![full_baseline(9, 1, vec![transform_payload(0.0)])],
            ),
        );

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                2,
                vec![
                    // Applies: a fresh spawn.
                    full_baseline(11, 5, vec![transform_payload(1.0)]),
                    // Does not apply: entity 12 has no held baseline.
                    delta(12, 99, 100, vec![transform_payload(9.0)]),
                    // Applies: despawn of mapped entity 9.
                    EntityRecord::Despawn {
                        network_id: 9,
                        tombstone_id: 7,
                        reason: 0,
                    },
                ],
            ),
        );
        let ack = out.ack.expect("accepted");
        assert_eq!(
            ack.entity_baselines,
            vec![(11, 5)],
            "only the applied baseline acked"
        );
        assert_eq!(
            ack.despawn_tombstones,
            vec![(9, 7)],
            "applied despawn acked"
        );
        assert!(
            client.is_pending_repair(NetworkId(12)),
            "unappliable delta -> pending"
        );
        assert_eq!(out.refresh_requests.len(), 1);
    }

    // --- A full baseline applies the rotation quaternion through the glam-aware
    // engine conversion (seam check: wire [x,y,z,w] -> glam Quat). ---
    #[test]
    fn full_baseline_applies_rotation_through_glam_conversion() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        let q = Quat::from_xyzw(0.182_574_2, 0.365_148_4, 0.547_722_6, 0.730_296_8).normalize();
        let payload = ComponentPayload::Transform(WireTransform {
            position: [1.0, 2.0, 3.0],
            rotation: [q.x, q.y, q.z, q.w],
            scale: [1.0, 1.0, 1.0],
        });
        client.apply_snapshot(
            &mut registry,
            &snapshot(0, 1, vec![full_baseline(7, 1, vec![payload])]),
        );
        let id = *client.map().get(&NetworkId(7)).unwrap();
        let t = registry.get_component::<Transform>(id).unwrap();
        assert!(
            t.rotation.angle_between(q) < 1e-4,
            "rotation survives the seam"
        );
    }

    // --- encode_client_messages: an ack-with-refresh outcome encodes the ack first,
    // then each refresh, all as ClientMessage envelopes. ---
    #[test]
    fn encode_client_messages_emits_ack_then_refreshes() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                1,
                vec![full_baseline(7, 1, vec![transform_payload(0.0)])],
            ),
        );
        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                2,
                vec![
                    delta(7, 1, 2, vec![transform_payload(1.0)]),
                    delta(8, 99, 100, vec![transform_payload(1.0)]),
                ],
            ),
        );
        let buffers = encode_client_messages(&out);
        assert_eq!(buffers.len(), 2, "ack + one refresh");
        // First is the ack, second is the refresh, both decode as ClientMessage.
        let first: ClientMessage = postretro_net::wire::decode(&buffers[0]).expect("ack decodes");
        assert!(matches!(first, ClientMessage::Ack(_)));
        let second: ClientMessage =
            postretro_net::wire::decode(&buffers[1]).expect("refresh decodes");
        assert!(matches!(second, ClientMessage::BaselineRefresh(_)));
    }

    // --- Interpolation buffer is fed by apply, keyed by server tick, and isolated
    // per NetworkId across two distinct entities. ---
    #[test]
    fn apply_feeds_interpolation_buffer_keyed_by_server_tick() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        // Two snapshots for entity 7 at server ticks 100 and 110, x = 0 then 10.
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(7, 1, vec![transform_payload(0.0)])],
            ),
        );
        client.apply_snapshot(
            &mut registry,
            &snapshot(1, 110, vec![delta(7, 1, 2, vec![transform_payload(10.0)])]),
        );

        // The buffer brackets render tick 105 -> interpolated midpoint x = 5.0.
        assert_eq!(
            client.presented_source(NetworkId(7), 105.0),
            Some(PoseSource::Interpolated)
        );
    }

    // --- sample_into_registry writes an *alpha-invariant* presented pose: the buffer
    // already resolved the final frame pose at the correct server-time target, so the
    // remote-presentation write collapses previous == current and the render-stage
    // interpolated_transform blend reproduces it verbatim at any alpha (the sim
    // sub-tick alpha is an unrelated time base and must not re-blend it). Continuity:
    // stepping the render tick forward advances the presented pose along the buffer's
    // own smooth trajectory (not via the render blend). ---
    #[test]
    fn sample_into_registry_presents_alpha_invariant_pose() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(7, 1, vec![transform_payload(0.0)])],
            ),
        );
        client.apply_snapshot(
            &mut registry,
            &snapshot(1, 110, vec![delta(7, 1, 2, vec![transform_payload(10.0)])]),
        );
        let id = *client.map().get(&NetworkId(7)).unwrap();

        // First present at render tick 102 -> interpolated x = 2.0. The pose must be
        // identical at alpha = 0.0, 0.5, and 1.0: the buffer's resolved pose is shown
        // verbatim, never re-blended by the render alpha.
        let stats = client.sample_into_registry(&mut registry, 102.0, 0.0);
        assert_eq!(stats.presented, 1, "one remote entity presented");
        assert_eq!(stats.held_newest, 0, "bracketed pose did not starve");
        let at_zero = registry.interpolated_transform(id, 0.0).unwrap();
        let at_half = registry.interpolated_transform(id, 0.5).unwrap();
        let at_one = registry.interpolated_transform(id, 1.0).unwrap();
        assert!(
            (at_one.position.x - 2.0).abs() < EPSILON,
            "presented pose is the buffer's resolved x = 2.0"
        );
        assert!(
            (at_zero.position.x - at_one.position.x).abs() < EPSILON,
            "alpha=0.0 pose equals alpha=1.0 pose (alpha-invariant)"
        );
        assert!(
            (at_half.position.x - at_one.position.x).abs() < EPSILON,
            "alpha=0.5 pose equals alpha=1.0 pose (alpha-invariant)"
        );
        // Rotation and scale are likewise alpha-invariant (previous == current).
        assert!(at_zero.rotation.abs_diff_eq(at_one.rotation, EPSILON));
        assert!((at_zero.scale - at_one.scale).length() < EPSILON);

        // Second present at render tick 106 -> the buffer's own trajectory advances the
        // presented pose to x = 6.0, still alpha-invariant. Continuity comes from the
        // buffer, not from the render blend carrying a prior pose.
        let stats = client.sample_into_registry(&mut registry, 106.0, 0.0);
        assert_eq!(stats.presented, 1, "one remote entity presented");
        assert_eq!(stats.held_newest, 0, "bracketed pose did not starve");
        let next_zero = registry.interpolated_transform(id, 0.0).unwrap();
        let next_one = registry.interpolated_transform(id, 1.0).unwrap();
        assert!((next_one.position.x - 6.0).abs() < EPSILON);
        assert!(
            (next_zero.position.x - next_one.position.x).abs() < EPSILON,
            "still alpha-invariant after a second present"
        );
    }

    #[test]
    fn remote_walk_rate_tracks_presented_speed_with_continuous_rebase() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(7, 1, vec![transform_payload(0.0)])],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).expect("remote is mapped");
        let mut mesh = unresolved_mesh();
        mesh.animation
            .as_mut()
            .expect("animated mesh")
            .current_state = "locomotion".into();
        registry.set_component(id, mesh).expect("attach mesh");
        resolve_pending_animation_stamps(&mut registry, 0.0);
        // 60 m/s is the reference speed: the first presented segment is half-speed
        // (5 m over 10 server ticks), then the next is full speed (10 m).
        client.cache_remote_enemy_walk_playback(
            NetworkId(7),
            Some((60.0, "locomotion".into(), None)),
        );

        client.apply_snapshot(
            &mut registry,
            &snapshot(1, 110, vec![delta(7, 1, 2, vec![transform_payload(5.0)])]),
        );
        client.sample_into_registry(&mut registry, 105.0, 1.0);
        let half_speed = registry
            .get_component::<MeshComponent>(id)
            .expect("mesh")
            .animation
            .as_ref()
            .expect("animation")
            .clone();
        assert!((half_speed.rate - 0.5).abs() < EPSILON);

        client.apply_snapshot(
            &mut registry,
            &snapshot(2, 120, vec![delta(7, 2, 3, vec![transform_payload(15.0)])]),
        );
        let before = registry
            .get_component::<MeshComponent>(id)
            .expect("mesh")
            .animation
            .as_ref()
            .expect("animation")
            .scaled_elapsed(2.0);
        client.sample_into_registry(&mut registry, 115.0, 2.0);
        let full_speed = registry
            .get_component::<MeshComponent>(id)
            .expect("mesh")
            .animation
            .as_ref()
            .expect("animation");
        let expected_full_rate =
            (10.0 / (10.0 * crate::netcode::SERVER_TICK_MICROS as f32 / 1_000_000.0) / 60.0)
                .clamp(RATE_MIN, RATE_MAX);
        assert!((full_speed.rate - expected_full_rate).abs() < EPSILON);
        assert!(
            (full_speed.scaled_elapsed(2.0) - before).abs() < f64::EPSILON,
            "the segment-rate change must not jump clip-local time"
        );
    }

    #[test]
    fn remote_locomotion_rate_precedence_matrix_is_override_then_derived_then_fallback() {
        struct Case {
            label: &'static str,
            override_speed: Option<f32>,
            derived_speed: Option<f32>,
            speed_scale: bool,
            expected: f32,
        }
        let measured = 3.0_f32;
        let move_speed = 3.5_f32;
        let cases = [
            Case {
                label: "override wins over derived",
                override_speed: Some(4.0),
                derived_speed: Some(2.5),
                speed_scale: true,
                expected: measured / 4.0,
            },
            Case {
                label: "derived clip speed",
                override_speed: None,
                derived_speed: Some(2.5),
                speed_scale: true,
                expected: measured / 2.5,
            },
            Case {
                label: "E10 move-speed fallback",
                override_speed: None,
                derived_speed: None,
                speed_scale: true,
                expected: measured / move_speed,
            },
            Case {
                label: "speedScale false",
                override_speed: Some(4.0),
                derived_speed: Some(2.5),
                speed_scale: false,
                expected: 1.0,
            },
        ];

        for case in cases {
            let mut registry = EntityRegistry::new();
            let mut client = ClientReplication::new();
            client.apply_snapshot(
                &mut registry,
                &snapshot(
                    0,
                    100,
                    vec![full_baseline(7, 1, vec![transform_payload(0.0)])],
                ),
            );
            let id = *client.map().get(&NetworkId(7)).expect("remote is mapped");
            let mut mesh = unresolved_mesh();
            let animation = mesh.animation.as_mut().unwrap();
            animation.current_state = "locomotion".to_string();
            animation.speed_scale = case.speed_scale;
            animation.states.get_mut("locomotion").unwrap().travel_speed = case.override_speed;
            registry.set_component(id, mesh).unwrap();
            client.cache_remote_enemy_walk_playback(
                NetworkId(7),
                Some((move_speed, "locomotion".to_string(), case.derived_speed)),
            );

            client.update_remote_enemy_walk_playback_rate(
                &mut registry,
                NetworkId(7),
                id,
                measured,
                1.0,
            );
            let actual = registry
                .get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .rate;
            let expected = case.expected.clamp(RATE_MIN, RATE_MAX);
            assert!(
                (actual - expected).abs() <= EPSILON,
                "{}: expected {expected}, got {actual}",
                case.label
            );
        }
    }

    #[test]
    fn remote_non_walk_state_leaves_animation_component_unchanged() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(7, 1, vec![transform_payload(0.0)])],
            ),
        );
        client.apply_snapshot(
            &mut registry,
            &snapshot(1, 110, vec![delta(7, 1, 2, vec![transform_payload(5.0)])]),
        );
        let id = *client.map().get(&NetworkId(7)).expect("remote is mapped");
        let mut mesh = unresolved_mesh();
        mesh.animation
            .as_mut()
            .expect("animated mesh")
            .current_state = "attack".into();
        registry.set_component(id, mesh).expect("attach mesh");
        resolve_pending_animation_stamps(&mut registry, 0.0);
        client.cache_remote_enemy_walk_playback(
            NetworkId(7),
            Some((60.0, "locomotion".into(), None)),
        );

        let before = registry
            .get_component::<MeshComponent>(id)
            .expect("mesh")
            .clone();
        client.sample_into_registry(&mut registry, 105.0, 1.0);
        let after = registry.get_component::<MeshComponent>(id).expect("mesh");
        assert_eq!(after, &before, "non-walk state must not rebase or write");
    }

    // Regression: rotating mover carry made stationary remote riders animate and face along their orbit.
    #[test]
    fn remote_player_orbiting_with_zero_intrinsic_velocity_stays_idle_and_facing() {
        const FACING_YAW: f32 = 0.6;

        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![
                        transform_payload_at([3.0, 0.0, 0.0], FACING_YAW),
                        movement_payload_with_velocity([0.0, 0.0, 0.0]),
                    ],
                )],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).expect("remote mapped");
        registry.set_component(id, remote_player_mesh()).unwrap();
        client.cache_remote_player_locomotion(
            NetworkId(7),
            Some(RemotePlayerLocomotionReference::idle_and_walk_forward(60.0)),
        );

        // The player stays still relative to the platform while their world Transform
        // sweeps a quarter-orbit around its pivot. The sparse delta intentionally
        // omits PlayerMovementState, as normal delta replication does for zero velocity.
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                110,
                vec![delta(
                    7,
                    1,
                    2,
                    vec![transform_payload_at([0.0, 0.0, 3.0], FACING_YAW)],
                )],
            ),
        );
        client.sample_into_registry(&mut registry, 105.0, 1.0);

        let animation = registry
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(animation.current_state, "idle");
        assert!(animation.previous_state.is_none());
        assert!(
            !client
                .presented_player_inputs()
                .heading_yaws
                .contains_key(&NetworkId(7)),
            "zero intrinsic velocity falls back to the replicated facing Transform"
        );
        let (presented_yaw, _, _) = registry
            .interpolated_transform(id, 0.5)
            .unwrap()
            .rotation
            .to_euler(glam::EulerRot::YXZ);
        assert!((presented_yaw - FACING_YAW).abs() <= EPSILON);

        for (sequence, baseline_ref, new_baseline_id) in [(2, 2, 3), (3, 3, 4)] {
            client.apply_snapshot(
                &mut registry,
                &snapshot(
                    sequence,
                    110 + sequence,
                    vec![delta(
                        7,
                        baseline_ref,
                        new_baseline_id,
                        vec![mesh_animation_payload("idle")],
                    )],
                ),
            );
            client.sample_into_registry(&mut registry, 105.0, 1.0);
            let animation = registry
                .get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap();
            assert_eq!(animation.current_state, "idle");
            assert!(animation.previous_state.is_none());
        }
    }

    #[test]
    fn remote_player_locomotion_and_heading_use_intrinsic_velocity_over_orbit_motion() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![
                        transform_payload_at([3.0, 0.0, 0.0], 0.75),
                        movement_payload_with_velocity([0.0, 0.0, -60.0]),
                    ],
                )],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).expect("remote mapped");
        registry.set_component(id, remote_player_mesh()).unwrap();
        client.cache_remote_player_locomotion(
            NetworkId(7),
            Some(RemotePlayerLocomotionReference::idle_and_walk_forward(60.0)),
        );
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                110,
                vec![delta(
                    7,
                    1,
                    2,
                    vec![
                        transform_payload_at([0.0, 0.0, 3.0], 0.75),
                        movement_payload_with_velocity([0.0, 0.0, -60.0]),
                    ],
                )],
            ),
        );
        client.sample_into_registry(&mut registry, 105.0, 1.0);

        let animation = registry
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(animation.current_state, "walk_forward");
        assert!((animation.rate - 1.0).abs() <= EPSILON);
        let heading_yaw = client
            .presented_player_inputs()
            .heading_yaws
            .get(&NetworkId(7))
            .copied()
            .expect("nonzero intrinsic velocity supplies player travel heading");
        assert!(
            heading_yaw.abs() <= EPSILON,
            "-Z intrinsic velocity is yaw zero"
        );

        // A movement-only idle delta must update presentation even though the
        // carried Transform has no new sample this snapshot.
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                111,
                vec![delta(
                    7,
                    2,
                    3,
                    vec![movement_payload_with_velocity([0.0, 0.0, 0.0])],
                )],
            ),
        );
        client.sample_into_registry(&mut registry, 111.0, 1.02);
        assert_eq!(
            registry
                .get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state,
            "idle"
        );
        assert!(
            !client
                .presented_player_inputs()
                .heading_yaws
                .contains_key(&NetworkId(7)),
            "movement-only idle clears the lower-body travel override"
        );
    }

    #[test]
    fn remote_player_locomotion_holds_an_authoritative_mismatch_until_the_derivation_agrees() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![
                        transform_payload(0.0),
                        movement_payload_with_velocity([60.0, 0.0, 0.0]),
                    ],
                )],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).expect("remote mapped");
        registry.set_component(id, remote_player_mesh()).unwrap();
        client.cache_remote_player_locomotion(
            NetworkId(7),
            Some(RemotePlayerLocomotionReference::idle_and_walk_forward(60.0)),
        );

        client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                110,
                vec![delta(
                    7,
                    1,
                    2,
                    vec![
                        transform_payload(10.0),
                        movement_payload_with_velocity([60.0, 0.0, 0.0]),
                    ],
                )],
            ),
        );
        client.sample_into_registry(&mut registry, 105.0, 1.0);
        let walking = registry
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(walking.current_state, "walk_forward");
        assert!((walking.rate - 1.0).abs() <= EPSILON);

        // Give the client-derived walk a resolved entry stamp so correction has an
        // outgoing pose to blend rather than a pending same-frame transition.
        resolve_pending_animation_stamps(&mut registry, 1.0);
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                111,
                vec![delta(7, 2, 3, vec![mesh_animation_payload("idle")])],
            ),
        );
        // Regression: normal frame ordering applies snapshots, then samples remote
        // interpolation before render resolves animation stamps. Velocity derivation
        // must not switch straight back to walk in this same frame.
        client.sample_into_registry(&mut registry, 111.0, 1.0);
        let corrected = registry
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(corrected.current_state, "idle");
        assert_eq!(corrected.previous_state.as_deref(), Some("walk_forward"));
        assert_eq!(corrected.states["idle"].crossfade_ms, 50.0);

        resolve_pending_animation_stamps(&mut registry, 1.0);
        client.sample_into_registry(&mut registry, 111.0, 1.02);
        assert_eq!(
            registry
                .get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state,
            "idle",
            "client locomotion holds through the configured server crossfade"
        );
        resolve_pending_animation_stamps(&mut registry, 1.06);
        client.sample_into_registry(&mut registry, 111.0, 1.06);
        assert_eq!(
            registry
                .get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state,
            "idle",
            "the host's state outlasts its crossfade: reverting to the lagging \
             prediction would only be corrected again by the next delta"
        );

        // The host comes round to walking; the client already derives walk, so
        // the correction releases and velocity derivation resumes.
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                3,
                112,
                vec![delta(7, 3, 4, vec![mesh_animation_payload("walk_forward")])],
            ),
        );
        resolve_pending_animation_stamps(&mut registry, 1.1);
        client.sample_into_registry(&mut registry, 112.0, 1.1);
        assert_eq!(
            registry
                .get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state,
            "walk_forward"
        );
        assert!(
            client.remote_player_locomotion[&NetworkId(7)]
                .authoritative_correction_state
                .is_none()
        );
    }

    /// [`remote_player_mesh`] plus resolved states named `extra`.
    fn remote_player_mesh_with(extra: &[&str]) -> MeshComponent {
        let mut mesh = remote_player_mesh();
        let animation = mesh.animation.as_mut().unwrap();
        for (index, name) in extra.iter().enumerate() {
            let mut state = animation.states["walk_forward"].clone();
            state.clip = name.to_string();
            state.clip_index = Some(2 + index);
            animation.states.insert(name.to_string(), state);
        }
        mesh
    }

    fn remote_player_backpedal_mesh() -> MeshComponent {
        remote_player_mesh_with(&["walk_backward"])
    }

    /// Map remote player 7 facing yaw zero (-Z), install `mesh`, and buffer it
    /// travelling at `velocity` across snapshots at ticks 100 and 110.
    fn map_remote_player(
        mesh: MeshComponent,
        declared: &[&str],
        velocity: [f32; 3],
        speeds: (f32, f32),
    ) -> (EntityRegistry, ClientReplication, EntityId) {
        map_remote_player_in(mesh, declared, velocity, WireMovementState::Normal, speeds)
    }

    /// [`map_remote_player`] with every snapshot replicating `movement_state`.
    fn map_remote_player_in(
        mesh: MeshComponent,
        declared: &[&str],
        velocity: [f32; 3],
        movement_state: WireMovementState,
        (walk_speed, run_speed): (f32, f32),
    ) -> (EntityRegistry, ClientReplication, EntityId) {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![
                        transform_payload_at([0.0, 0.0, 0.0], 0.0),
                        movement_payload_with(velocity, 0.0, movement_state),
                    ],
                )],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).expect("remote mapped");
        registry.set_component(id, mesh).unwrap();
        client.cache_remote_player_locomotion(
            NetworkId(7),
            Some(RemotePlayerLocomotionReference::declaring(
                declared, walk_speed, run_speed,
            )),
        );
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                110,
                vec![delta(
                    7,
                    1,
                    2,
                    vec![
                        transform_payload_at([1.0, 0.0, 1.0], 0.0),
                        movement_payload_with(velocity, 0.0, movement_state),
                    ],
                )],
            ),
        );
        (registry, client, id)
    }

    /// [`map_remote_player`], then present it at render tick 105.
    fn present_remote_player(
        mesh: MeshComponent,
        declared: &[&str],
        velocity: [f32; 3],
        speeds: (f32, f32),
    ) -> (EntityRegistry, ClientReplication, EntityId) {
        let (mut registry, mut client, id) = map_remote_player(mesh, declared, velocity, speeds);
        client.sample_into_registry(&mut registry, 105.0, 1.0);
        (registry, client, id)
    }

    fn presented_state_and_heading(
        registry: &EntityRegistry,
        client: &ClientReplication,
        id: EntityId,
    ) -> (String, f32) {
        let state = registry
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .current_state
            .clone();
        let heading = client.presented_player_inputs().heading_yaws[&NetworkId(7)];
        (state, heading)
    }

    #[test]
    fn remote_player_backpedal_keeps_legs_facing_aim() {
        // Aim faces -Z; +Z travel is a backpedal.
        let (registry, client, id) = present_remote_player(
            remote_player_backpedal_mesh(),
            &["idle", "walk_forward", "walk_backward"],
            [0.0, 0.0, 60.0],
            (60.0, 60.0),
        );
        let (state, heading) = presented_state_and_heading(&registry, &client, id);
        assert_eq!(state, "walk_backward");
        assert!(
            heading.abs() <= EPSILON,
            "legs face aim, not travel: {heading}"
        );

        // Without a backward clip the forward clip plays, legs still forward.
        let (registry, client, id) = present_remote_player(
            remote_player_mesh(),
            &["idle", "walk_forward"],
            [0.0, 0.0, 60.0],
            (60.0, 60.0),
        );
        let (state, heading) = presented_state_and_heading(&registry, &client, id);
        assert_eq!(state, "walk_forward");
        assert!(
            heading.abs() <= EPSILON,
            "legs face aim, not travel: {heading}"
        );
    }

    #[test]
    fn remote_player_prediction_agrees_with_host_selection_across_tiers_and_directions() {
        use postretro_foundation::PlayerMovementComponent;

        let extra = ["walk_backward", "run_forward", "run_backward"];
        let declared = [
            "idle",
            "walk_forward",
            "walk_backward",
            "run_forward",
            "run_backward",
        ];
        // Aim faces -Z. Ground walk 7, run 11.
        for (velocity, expected) in [
            ([0.0, 0.0, 5.0], "walk_backward"),
            ([0.0, 0.0, 10.0], "run_backward"),
            // 8.5 m/s sits inside the walk/run band; from rest it stays walk
            // (the run tier enters above 9.6).
            ([6.0, 0.0, 6.0], "walk_backward"),
            ([8.0, 0.0, 8.0], "run_backward"),
            ([0.0, 0.0, -10.0], "run_forward"),
            ([-3.0, 0.0, -4.0], "walk_forward"),
            // Pure strafe from rest sits in the dead zone and starts forward.
            ([5.0, 0.0, 0.0], "walk_forward"),
        ] {
            let mut host = EntityRegistry::new();
            let pawn = host.spawn(Transform::default());
            let mut movement = PlayerMovementComponent::from_descriptor(
                &crate::snapshot_regression_harness_test::player_descriptor(),
            );
            movement.velocity = Vec3::from(velocity);
            host.set_component(pawn, movement).unwrap();
            host.set_component(pawn, remote_player_mesh_with(&extra))
                .unwrap();
            crate::sim::update_player_animation_locomotion(
                &mut host,
                &crate::scripting_systems::hit_zones::HitZoneStore::new(),
                0.0,
            );
            let host_state = host
                .get_component::<MeshComponent>(pawn)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state
                .clone();
            assert_eq!(host_state, expected, "host at {velocity:?}");

            let (mut registry, mut client, id) = present_remote_player(
                remote_player_mesh_with(&extra),
                &declared,
                velocity,
                (7.0, 11.0),
            );
            let (client_state, _) = presented_state_and_heading(&registry, &client, id);
            assert_eq!(client_state, host_state, "client at {velocity:?}");

            resolve_pending_animation_stamps(&mut registry, 1.0);
            client.apply_snapshot(
                &mut registry,
                &snapshot(
                    2,
                    111,
                    vec![delta(7, 2, 3, vec![mesh_animation_payload(&host_state)])],
                ),
            );
            assert!(
                client.remote_player_locomotion[&NetworkId(7)]
                    .authoritative_correction_state
                    .is_none(),
                "the host state at {velocity:?} needs no correction"
            );
        }
    }

    #[test]
    fn remote_player_prediction_agrees_with_host_selection_for_crouch_and_slide() {
        use postretro_foundation::{MovementState, PlayerMovementComponent};

        let extra = [
            "walk_backward",
            "run_forward",
            "crouch_idle",
            "crouch_forward",
            "crouch_backward",
        ];
        let declared = [
            "idle",
            "walk_forward",
            "walk_backward",
            "run_forward",
            "crouch_idle",
            "crouch_forward",
            "crouch_backward",
        ];
        // Crouch states name no travel speed, so both peers rate-scale against
        // `ground.speed.crouch` (3).
        let mesh = || {
            let mut mesh = remote_player_mesh_with(&extra);
            let animation = mesh.animation.as_mut().unwrap();
            for name in ["crouch_forward", "crouch_backward"] {
                animation.states.get_mut(name).unwrap().travel_speed = None;
            }
            mesh
        };
        let crouching = (
            MovementState::Crouching { eye_current: 0.3 },
            WireMovementState::Crouching { eye_current: 0.3 },
        );
        let sliding = (
            MovementState::Sliding {
                elapsed_ms: 40.0,
                boost: Vec3::NEG_Z * 4.0,
                eye_current: 0.3,
            },
            WireMovementState::Sliding {
                elapsed_ms: 40.0,
                boost: [0.0, 0.0, -4.0],
                eye_current: 0.3,
                floor_normal: None,
            },
        );
        let standing = (MovementState::Normal, WireMovementState::Normal);
        // Aim faces -Z.
        for ((host_movement_state, wire_movement_state), velocity, expected, rate) in [
            (crouching, [0.0, 0.0, 0.0], "crouch_idle", 1.0),
            (crouching, [0.0, 0.0, -2.4], "crouch_forward", 0.8),
            (crouching, [0.0, 0.0, 2.4], "crouch_backward", 0.8),
            // Posture, not speed, picks the tier: a crouched body never runs.
            (crouching, [0.0, 0.0, -10.0], "crouch_forward", 1.5),
            (sliding, [0.0, 0.0, -12.0], "crouch_idle", 1.0),
            // Standing selects as before; run_forward's 60 m/s stride clamps
            // the rate to its 0.5 floor.
            (standing, [0.0, 0.0, -10.0], "run_forward", 0.5),
        ] {
            let mut host = EntityRegistry::new();
            let pawn = host.spawn(Transform::default());
            let mut movement = PlayerMovementComponent::from_descriptor(
                &crate::snapshot_regression_harness_test::player_descriptor(),
            );
            movement.velocity = Vec3::from(velocity);
            movement.movement_state = host_movement_state;
            host.set_component(pawn, movement).unwrap();
            host.set_component(pawn, mesh()).unwrap();
            crate::sim::update_player_animation_locomotion(
                &mut host,
                &crate::scripting_systems::hit_zones::HitZoneStore::new(),
                0.0,
            );
            let host_animation = host
                .get_component::<MeshComponent>(pawn)
                .unwrap()
                .animation
                .clone()
                .unwrap();
            assert_eq!(
                host_animation.current_state, expected,
                "host {host_movement_state:?} at {velocity:?}"
            );

            let (mut registry, mut client, id) = map_remote_player_in(
                mesh(),
                &declared,
                velocity,
                wire_movement_state,
                (7.0, 11.0),
            );
            client.sample_into_registry(&mut registry, 105.0, 1.0);
            let client_animation = registry
                .get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .clone()
                .unwrap();
            assert_eq!(
                client_animation.current_state, host_animation.current_state,
                "client {wire_movement_state:?} at {velocity:?}"
            );
            assert!(
                (client_animation.rate - host_animation.rate).abs() <= EPSILON
                    && (client_animation.rate - rate).abs() <= EPSILON,
                "{expected} rate: host {} client {}",
                host_animation.rate,
                client_animation.rate
            );

            resolve_pending_animation_stamps(&mut registry, 1.0);
            client.apply_snapshot(
                &mut registry,
                &snapshot(
                    2,
                    111,
                    vec![delta(
                        7,
                        2,
                        3,
                        vec![mesh_animation_payload(&host_animation.current_state)],
                    )],
                ),
            );
            assert!(
                client.remote_player_locomotion[&NetworkId(7)]
                    .authoritative_correction_state
                    .is_none(),
                "the host state for {wire_movement_state:?} needs no correction"
            );
        }
    }

    /// A remote player declaring walk, run and the jump sequence, the host's clip
    /// store for its model, and the client's reference: walk 7, run 11,
    /// `jump_up` 0.4 s, `jump_down` 0.3 s. Both peers read the one-shots'
    /// lengths from the model's clips: the host per tick, the client precomputed
    /// at materialization.
    fn jump_sequence_fixture() -> (
        MeshComponent,
        crate::scripting_systems::hit_zones::HitZoneStore,
        RemotePlayerLocomotionReference,
    ) {
        use crate::scripting_systems::hit_zones::{HitZoneStore, ModelHitZones};
        use postretro_model::skeleton::{AnimationClip, Skeleton};

        let extra = ["run_forward", "jump_up", "jump_loop", "jump_down"];
        let declared = [
            "idle",
            "walk_forward",
            "run_forward",
            "jump_up",
            "jump_loop",
            "jump_down",
        ];
        let mesh = remote_player_mesh_with(&extra);
        let clip = |name: &str, duration| AnimationClip {
            name: name.to_string(),
            duration,
            joints: Vec::new(),
            travel_speed: None,
        };
        let mut hit_zones = HitZoneStore::new();
        hit_zones.insert_for_test(
            postretro_model::ModelHandle::from(mesh.model.as_str()),
            ModelHitZones::for_test(
                std::sync::Arc::new(Skeleton { joints: Vec::new() }),
                std::sync::Arc::new(vec![
                    clip("idle", 1.0),
                    clip("walk_forward", 1.0),
                    clip("run_forward", 1.0),
                    clip("jump_up", 0.4),
                    clip("jump_loop", 1.0),
                    clip("jump_down", 0.3),
                ]),
                Vec::new(),
                HashMap::new(),
                None,
                Vec::new(),
                Default::default(),
            ),
        );
        let mut reference = RemotePlayerLocomotionReference::declaring(&declared, 7.0, 11.0);
        reference.clip_durations =
            HashMap::from([("jump_up".to_string(), 0.4), ("jump_down".to_string(), 0.3)]);
        (mesh, hit_zones, reference)
    }

    /// One host tick of player movement: `air_ticks` zero is grounded.
    fn host_movement(
        air_ticks: u32,
        jumped: bool,
        velocity: Vec3,
    ) -> postretro_foundation::PlayerMovementComponent {
        use postretro_foundation::{GroundRef, PlayerMovementComponent};
        let mut movement = PlayerMovementComponent::from_descriptor(
            &crate::snapshot_regression_harness_test::player_descriptor(),
        );
        movement.ground = if air_ticks == 0 {
            GroundRef::World
        } else {
            GroundRef::Airborne
        };
        movement.air_ticks = air_ticks;
        movement.jump_spent = jumped;
        movement.velocity = velocity;
        movement
    }

    #[test]
    fn remote_player_prediction_agrees_with_host_selection_across_the_jump_sequence() {
        let (mesh, hit_zones, reference) = jump_sequence_fixture();
        let mut host = EntityRegistry::new();
        let pawn = host.spawn(Transform::default());
        host.set_component(pawn, mesh.clone()).unwrap();
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![
                        transform_payload_at([0.0, 0.0, 0.0], 0.0),
                        movement_payload(),
                    ],
                )],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).expect("remote mapped");
        registry.set_component(id, mesh).unwrap();
        client.cache_remote_player_locomotion(NetworkId(7), Some(reference));

        let walking = Vec3::NEG_Z * 5.0;
        let running = Vec3::NEG_Z * 10.0;
        // (animation clock, airborne ticks — 0 is grounded —, jump spent,
        // velocity, the state both peers select). Aim faces -Z.
        let sequence = [
            (0.0, 0, false, walking, "walk_forward"),
            (0.02, 1, true, walking + Vec3::Y * 6.0, "jump_up"),
            (0.2, 12, true, walking + Vec3::Y * 0.5, "jump_up"),
            (0.22, 13, true, walking - Vec3::Y * 0.5, "jump_loop"),
            (0.5, 0, false, Vec3::NEG_Z * 3.0, "jump_down"),
            (0.79, 0, false, Vec3::NEG_Z * 3.0, "jump_down"),
            (0.81, 0, false, Vec3::NEG_Z * 3.0, "walk_forward"),
            // Walking off a ledge keeps the stride, then falls; a running
            // landing returns straight to the stride.
            (1.0, 1, false, walking - Vec3::Y * 0.3, "walk_forward"),
            (1.13, 8, false, walking - Vec3::Y * 2.4, "walk_forward"),
            (1.15, 9, false, walking - Vec3::Y * 2.7, "jump_loop"),
            (1.5, 0, false, running, "run_forward"),
        ];
        let mut host_state: Option<String> = None;
        for (tick, (time, air_ticks, jumped, velocity, expected)) in (1u32..).zip(sequence) {
            let movement = host_movement(air_ticks, jumped, velocity);
            let wire = crate::movement_state::movement_state_to_wire(&movement, 0.0);
            host.set_component(pawn, movement).unwrap();
            crate::sim::update_player_animation_locomotion(&mut host, &hit_zones, time);
            let host_animation = host
                .get_component::<MeshComponent>(pawn)
                .unwrap()
                .animation
                .clone()
                .unwrap();
            assert_eq!(host_animation.current_state, expected, "host at {time}");

            // Each snapshot carries the host's state from the tick before, which
            // the client already derived: no correction.
            let mut components = vec![
                transform_payload_at([0.0, 0.0, -0.1 * tick as f32], 0.0),
                ComponentPayload::PlayerMovementState(wire),
            ];
            components.extend(host_state.as_deref().map(mesh_animation_payload));
            client.apply_snapshot(
                &mut registry,
                &snapshot(tick, 100 + tick, vec![delta(7, tick, tick + 1, components)]),
            );
            client.sample_into_registry(&mut registry, f64::from(100 + tick), time);
            assert!(
                client.remote_player_locomotion[&NetworkId(7)]
                    .authoritative_correction_state
                    .is_none(),
                "the host state before {time} needs no correction"
            );
            let client_animation = registry
                .get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .clone()
                .unwrap();
            assert_eq!(
                client_animation.current_state, host_animation.current_state,
                "client at {time}"
            );
            assert_eq!(
                client_animation.locomotion_jump, host_animation.locomotion_jump,
                "jump memory at {time}"
            );
            if expected.starts_with("jump") {
                assert!(
                    (host_animation.rate - 1.0).abs() <= EPSILON
                        && (client_animation.rate - 1.0).abs() <= EPSILON,
                    "{expected} plays at the authored rate"
                );
            }
            host_state = Some(host_animation.current_state);
        }
    }

    /// One scripted host tick for [`present_jumps_at_snapshot_cadence`]:
    /// `air_ticks` zero is grounded.
    #[derive(Debug, Clone, Copy)]
    struct ScriptedTick {
        air_ticks: u32,
        jumped: bool,
        velocity: Vec3,
    }

    /// Host movement for a walk, a jump with a slow landing and a walk off,
    /// then a run into a bunny-hop chain whose landings last one grounded tick.
    /// Returns the script and the slow landing's tick index.
    fn jump_and_bunny_hop_script() -> (Vec<ScriptedTick>, usize) {
        const GRAVITY: f32 = 20.0;
        let dt = 1.0 / 60.0;
        let mut script = Vec::new();
        let ground = |script: &mut Vec<ScriptedTick>, ticks, speed: f32| {
            for _ in 0..ticks {
                script.push(ScriptedTick {
                    air_ticks: 0,
                    jumped: false,
                    velocity: Vec3::NEG_Z * speed,
                });
            }
        };
        let jump = |script: &mut Vec<ScriptedTick>, speed: f32, takeoff: f32| {
            let (mut height, mut vertical) = (0.0, takeoff);
            for air_ticks in 1.. {
                height += vertical * dt;
                if height <= 0.0 {
                    break;
                }
                script.push(ScriptedTick {
                    air_ticks,
                    jumped: true,
                    velocity: Vec3::new(0.0, vertical, -speed),
                });
                vertical -= GRAVITY * dt;
            }
        };
        ground(&mut script, 12, 5.0);
        jump(&mut script, 3.0, 6.0);
        let slow_landing = script.len();
        ground(&mut script, 40, 3.0);
        ground(&mut script, 10, 10.0);
        // Varied takeoff speeds vary the hop lengths, so the one grounded tick
        // falls both on and between snapshot ticks.
        for takeoff in [6.0, 6.1, 6.2, 6.3] {
            jump(&mut script, 10.0, takeoff);
            ground(&mut script, 1, 10.0);
        }
        ground(&mut script, 20, 10.0);
        (script, slow_landing)
    }

    /// What the client presented for a remote jumper over a host run at the
    /// real cadence.
    struct CadenceRun {
        /// Per host tick: the host's selected state.
        host_states: Vec<String>,
        /// Per client frame: render tick (in script ticks), presented state and
        /// footing, and whether a host correction was held.
        frames: Vec<(f64, String, crate::sim::LocomotionFooting, bool)>,
    }

    /// Run `script` on a host at 60 Hz, sending every second tick's snapshot
    /// through the client's apply path, and present it one frame per host tick
    /// at a render tick 4.25 ticks behind the host, the animation clock tied to
    /// the render tick. `host_anim_time` maps a script tick to the host
    /// animation clock its frame ran with.
    fn present_jumps_at_snapshot_cadence(
        script: &[ScriptedTick],
        host_anim_time: impl Fn(usize) -> f64,
    ) -> CadenceRun {
        const BASE: u32 = 1000;
        const RENDER_LAG: f64 = 4.25;
        let dt = 1.0 / 60.0;
        let (mesh, hit_zones, reference) = jump_sequence_fixture();
        let mut host = EntityRegistry::new();
        let pawn = host.spawn(Transform::default());
        host.set_component(pawn, mesh.clone()).unwrap();
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        let first = script[0];
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                BASE,
                vec![full_baseline(
                    7,
                    1,
                    vec![
                        transform_payload_at([0.0, 0.0, 0.0], 0.0),
                        ComponentPayload::PlayerMovementState(
                            crate::movement_state::movement_state_to_wire(
                                &host_movement(first.air_ticks, first.jumped, first.velocity),
                                0.0,
                            ),
                        ),
                    ],
                )],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).expect("remote mapped");
        registry.set_component(id, mesh).unwrap();
        client.cache_remote_player_locomotion(NetworkId(7), Some(reference));

        let mut run = CadenceRun {
            host_states: Vec::new(),
            frames: Vec::new(),
        };
        let mut position = Vec3::ZERO;
        let mut baseline = 1;
        for (index, scripted) in script.iter().enumerate() {
            let server_tick = BASE + 1 + index as u32;
            let movement = host_movement(scripted.air_ticks, scripted.jumped, scripted.velocity);
            let wire = crate::movement_state::movement_state_to_wire(&movement, 0.0);
            host.set_component(pawn, movement).unwrap();
            crate::sim::update_player_animation_locomotion(
                &mut host,
                &hit_zones,
                host_anim_time(index),
            );
            let host_state = host
                .get_component::<MeshComponent>(pawn)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state
                .clone();
            position += scripted.velocity * dt;
            position.y = position.y.max(0.0);
            if server_tick.is_multiple_of(2) {
                client.apply_snapshot(
                    &mut registry,
                    &snapshot(
                        baseline,
                        server_tick,
                        vec![delta(
                            7,
                            baseline,
                            baseline + 1,
                            vec![
                                transform_payload_at(position.to_array(), 0.0),
                                ComponentPayload::PlayerMovementState(wire),
                                mesh_animation_payload(&host_state),
                            ],
                        )],
                    ),
                );
                baseline += 1;
            }
            run.host_states.push(host_state);

            let render_tick = f64::from(server_tick) - RENDER_LAG;
            let frame_anim_time = render_tick / 60.0;
            client.sample_into_registry(&mut registry, render_tick, frame_anim_time);
            resolve_pending_animation_stamps(&mut registry, frame_anim_time);
            let state = registry
                .get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state
                .clone();
            let footing = client
                .interp
                .presented_pose(NetworkId(7), render_tick)
                .expect("buffered")
                .stance
                .footing;
            let held = client.remote_player_locomotion[&NetworkId(7)]
                .authoritative_correction_state
                .is_some();
            run.frames
                .push((render_tick - f64::from(BASE + 1), state, footing, held));
        }
        run
    }

    /// The realistic-cadence properties every run must keep.
    fn assert_jumps_present_with_their_footing(run: &CadenceRun, script: &[ScriptedTick]) {
        use crate::sim::LocomotionFooting;
        let airborne =
            |footing: LocomotionFooting| matches!(footing, LocomotionFooting::Airborne { .. });
        let mut previous: Option<(&str, LocomotionFooting)> = None;
        let mut held_frames = 0;
        for (render_tick, state, footing, held) in &run.frames {
            let (render_tick, state, footing) = (*render_tick, state.as_str(), *footing);
            // A jump state shows only once the presented body has reached the
            // host tick that chose it: the footing presented with it agrees.
            match state {
                "jump_up" => assert!(
                    matches!(
                        footing,
                        LocomotionFooting::Airborne { vertical_speed, jumped: true, .. }
                            if vertical_speed > 0.0
                    ),
                    "jump_up over {footing:?} at {render_tick}"
                ),
                "jump_loop" => {
                    assert!(
                        airborne(footing),
                        "jump_loop over {footing:?} at {render_tick}"
                    )
                }
                "jump_down" => assert_eq!(
                    footing,
                    LocomotionFooting::Grounded,
                    "jump_down at {render_tick}"
                ),
                _ => {}
            }
            // A one-shot is entered only on the presented edge that starts it:
            // a takeoff (from the ground, or a shorter airborne stretch than the
            // last), or a touchdown. Never again after leaving it.
            if let Some((previous_state, previous_footing)) = previous
                && previous_state != state
            {
                match state {
                    "jump_up" => assert!(
                        match (previous_footing, footing) {
                            (
                                LocomotionFooting::Airborne {
                                    air_ticks: before, ..
                                },
                                LocomotionFooting::Airborne { air_ticks, .. },
                            ) => air_ticks < before,
                            _ => true,
                        },
                        "jump_up re-entered mid-air at {render_tick}"
                    ),
                    "jump_down" => assert!(
                        airborne(previous_footing),
                        "jump_down re-entered on the ground at {render_tick}"
                    ),
                    _ => {}
                }
            }
            // A held correction releases within a bounded number of frames.
            held_frames = if *held { held_frames + 1 } else { 0 };
            assert!(held_frames <= 2, "correction held past {render_tick}");
            previous = Some((state, footing));
        }
        // Every host takeoff shows `jump_up` on the client once presented,
        // bunny-hop re-takeoffs included.
        let takeoffs: Vec<usize> = (1..run.host_states.len())
            .filter(|&tick| {
                run.host_states[tick] == "jump_up" && run.host_states[tick - 1] != "jump_up"
            })
            .collect();
        assert_eq!(takeoffs.len(), 5, "one jump and four hops");
        for takeoff in takeoffs {
            assert!(
                run.frames.iter().any(|(render_tick, state, _, _)| {
                    state == "jump_up"
                        && *render_tick >= takeoff as f64
                        && *render_tick < takeoff as f64 + 6.0
                }),
                "the takeoff at tick {takeoff} never shows jump_up"
            );
        }
        // The chain exercises a grounded tick no snapshot carried: script tick
        // `i` is server tick 1001 + i, so an even index is never sent.
        assert!(
            (1..script.len()).any(|tick| {
                script[tick].air_ticks == 0
                    && script[tick - 1].air_ticks > 0
                    && script.get(tick + 1).is_some_and(|next| next.air_ticks == 1)
                    && tick.is_multiple_of(2)
            }),
            "some one-tick landing falls between snapshots"
        );
    }

    // Regression: host states applied on receipt led the presented body (jump_up
    // on the ground, jump_down mid-fall), a late resend re-entered a one-shot,
    // and a stepped presentation missed one-tick bunny-hop landings.
    #[test]
    fn remote_player_jumps_present_with_their_footing_at_snapshot_cadence() {
        let (script, slow_landing) = jump_and_bunny_hop_script();
        let run = present_jumps_at_snapshot_cadence(&script, |tick| tick as f64 / 60.0);
        assert!(
            run.host_states[slow_landing] == "jump_down",
            "the script's slow landing plays jump_down on the host"
        );
        assert_jumps_present_with_their_footing(&run, &script);
        assert!(
            run.frames
                .iter()
                .any(|(_, state, _, _)| state == "jump_down"),
            "the client plays the slow landing"
        );
    }

    #[test]
    fn remote_player_jumps_present_with_their_footing_through_a_host_hitch_at_landing() {
        // The host runs the ten ticks around the slow landing in one frame, so
        // they share its animation clock: its `jump_down` is stamped late and
        // outlasts the client's, which its resends must not restart.
        let (script, slow_landing) = jump_and_bunny_hop_script();
        let hitch = slow_landing - 1..=slow_landing + 8;
        let frame_end = (slow_landing + 8) as f64 / 60.0;
        let run = present_jumps_at_snapshot_cadence(&script, |tick| {
            if hitch.contains(&tick) {
                frame_end
            } else {
                tick as f64 / 60.0
            }
        });
        let host_jump_down = run.host_states.iter().filter(|s| *s == "jump_down").count();
        assert!(
            host_jump_down > 24,
            "the hitch stretches the host's jump_down well past the client's 18 ticks"
        );
        assert_jumps_present_with_their_footing(&run, &script);
    }

    // Regression: the held newest pose handing over to an older interpolated one
    // ran the footing backward, reading a held rise's grounded past as a landing.
    #[test]
    fn remote_player_held_to_interpolated_handover_does_not_land() {
        let (mesh, _, reference) = jump_sequence_fixture();
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        let movement = |air_ticks, jumped, velocity| {
            ComponentPayload::PlayerMovementState(crate::movement_state::movement_state_to_wire(
                &host_movement(air_ticks, jumped, velocity),
                0.0,
            ))
        };
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![
                        transform_payload_at([0.0, 0.0, 0.0], 0.0),
                        movement(0, false, Vec3::ZERO),
                    ],
                )],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).expect("remote mapped");
        registry.set_component(id, mesh).unwrap();
        client.cache_remote_player_locomotion(NetworkId(7), Some(reference));
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                102,
                vec![delta(
                    7,
                    1,
                    2,
                    vec![
                        transform_payload_at([0.0, 0.2, 0.0], 0.0),
                        movement(2, true, Vec3::Y * 6.0),
                    ],
                )],
            ),
        );
        let presented_state = |registry: &EntityRegistry| {
            registry
                .get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state
                .clone()
        };
        // Before time sync the newest pose holds: a takeoff.
        client.apply_held_remote_player_presentation(&mut registry, 1.0);
        assert_eq!(presented_state(&registry), "jump_up");
        resolve_pending_animation_stamps(&mut registry, 1.0);
        // Interpolation starts behind it, still on the ground: no landing.
        client.sample_into_registry(&mut registry, 101.0, 1.02);
        assert_eq!(presented_state(&registry), "idle");
        resolve_pending_animation_stamps(&mut registry, 1.02);
        // The presented takeoff plays as one.
        client.sample_into_registry(&mut registry, 102.0, 1.04);
        assert_eq!(presented_state(&registry), "jump_up");
    }

    #[test]
    fn remote_player_posture_steps_with_the_presented_sample() {
        // Crouched at tick 100, standing from tick 110: the presented posture is
        // the earlier sample's until the render target reaches the later one.
        let mesh = remote_player_mesh_with(&["crouch_idle", "crouch_forward"]);
        let declared = ["idle", "walk_forward", "crouch_idle", "crouch_forward"];
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        let velocity = [0.0, 0.0, -2.0];
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![
                        transform_payload_at([0.0, 0.0, 0.0], 0.0),
                        movement_payload_with(
                            velocity,
                            0.0,
                            WireMovementState::Crouching { eye_current: 0.3 },
                        ),
                    ],
                )],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).expect("remote mapped");
        registry.set_component(id, mesh).unwrap();
        client.cache_remote_player_locomotion(
            NetworkId(7),
            Some(RemotePlayerLocomotionReference::declaring(
                &declared, 7.0, 11.0,
            )),
        );
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                110,
                vec![delta(
                    7,
                    1,
                    2,
                    vec![
                        transform_payload_at([0.0, 0.0, -0.33], 0.0),
                        movement_payload_with(velocity, 0.0, WireMovementState::Normal),
                    ],
                )],
            ),
        );
        let presented_state = |registry: &EntityRegistry| {
            registry
                .get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state
                .clone()
        };
        client.sample_into_registry(&mut registry, 109.0, 1.0);
        assert_eq!(presented_state(&registry), "crouch_forward");
        resolve_pending_animation_stamps(&mut registry, 1.0);
        client.sample_into_registry(&mut registry, 110.0, 1.1);
        assert_eq!(presented_state(&registry), "walk_forward");
    }

    // A host one-shot refused because this client's phase has not reached it
    // releases a hold on the host's earlier state, so the client's own
    // derivation switches instead of playing the stale state through the jump.
    #[test]
    fn remote_player_refused_one_shot_releases_a_stale_hold() {
        let (mut registry, mut client, id) = present_remote_player(
            remote_player_mesh_with(&["run_forward", "jump_up", "jump_loop", "jump_down"]),
            &[
                "idle",
                "walk_forward",
                "run_forward",
                "jump_up",
                "jump_loop",
                "jump_down",
            ],
            [60.0, 0.0, 0.0],
            (60.0, 120.0),
        );
        let locomotion = client
            .remote_player_locomotion
            .get_mut(&NetworkId(7))
            .unwrap();
        locomotion.authoritative_correction_state = Some("run_forward".to_string());
        locomotion.queue_correction(200, "jump_up");
        client.apply_due_remote_player_correction(&mut registry, NetworkId(7), id, Some(200.0));
        let locomotion = &client.remote_player_locomotion[&NetworkId(7)];
        assert_eq!(locomotion.authoritative_correction_state, None);
        assert!(locomotion.pending_corrections.is_empty());
        assert_ne!(
            registry
                .get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state,
            "jump_up",
            "a refused one-shot is not entered"
        );
    }

    // Regression: a host state applied on receipt led the presented body by the
    // interpolation delay, so a remote jump played `jump_up` while the presented
    // body still stood and `jump_down` while it still fell.
    #[test]
    fn remote_player_host_state_waits_for_the_presented_tick() {
        let (mut registry, mut client, id) = present_remote_player(
            remote_player_mesh(),
            &["idle", "walk_forward"],
            [60.0, 0.0, 0.0],
            (60.0, 60.0),
        );
        let presented_state = |registry: &EntityRegistry| {
            registry
                .get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state
                .clone()
        };
        assert_eq!(presented_state(&registry), "walk_forward");
        resolve_pending_animation_stamps(&mut registry, 1.0);
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                111,
                vec![delta(7, 2, 3, vec![mesh_animation_payload("idle")])],
            ),
        );
        // The host's tick-111 state waits while the presentation is short of it.
        for render_tick in [105.0, 110.0, 110.99] {
            client.sample_into_registry(&mut registry, render_tick, 1.0);
            assert_eq!(
                presented_state(&registry),
                "walk_forward",
                "at render tick {render_tick}"
            );
        }
        client.sample_into_registry(&mut registry, 111.0, 1.0);
        assert_eq!(presented_state(&registry), "idle");
        assert_eq!(
            client.remote_player_locomotion[&NetworkId(7)]
                .authoritative_correction_state
                .as_deref(),
            Some("idle"),
            "applied, the correction holds as before"
        );

        // Before time sync names a render tick, the held presentation applies
        // whatever has arrived. Its held pose shows no travel, so it derives idle
        // until the host's walk corrects it.
        let (mut registry, mut client, held_id) = map_remote_player(
            remote_player_mesh(),
            &["idle", "walk_forward"],
            [60.0, 0.0, 0.0],
            (60.0, 60.0),
        );
        assert_eq!(held_id, id, "both fixtures map the avatar to one slot");
        client.apply_held_remote_player_presentation(&mut registry, 1.0);
        assert_eq!(presented_state(&registry), "idle");
        resolve_pending_animation_stamps(&mut registry, 1.0);
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                111,
                vec![delta(7, 2, 3, vec![mesh_animation_payload("walk_forward")])],
            ),
        );
        client.apply_held_remote_player_presentation(&mut registry, 1.02);
        assert_eq!(presented_state(&registry), "walk_forward");
        assert!(
            client.remote_player_locomotion[&NetworkId(7)]
                .pending_corrections
                .is_empty()
        );
    }

    #[test]
    fn remote_player_pending_host_states_collapse_resends_and_stay_bounded() {
        let mut locomotion = RemotePlayerLocomotion::new(
            RemotePlayerLocomotionReference::idle_and_walk_forward(60.0),
        );
        locomotion.queue_correction(10, "walk_forward");
        locomotion.queue_correction(12, "walk_forward");
        locomotion.queue_correction(14, "idle");
        let queued = |locomotion: &RemotePlayerLocomotion| {
            locomotion
                .pending_corrections
                .iter()
                .map(|pending| (pending.server_tick, pending.state.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            queued(&locomotion),
            [(10, "walk_forward".to_string()), (14, "idle".to_string())],
            "a resend keeps the tick its state began"
        );
        assert_eq!(locomotion.take_due_correction(Some(9.5)), None);
        // Both are due by tick 14; the older is superseded.
        assert_eq!(
            locomotion.take_due_correction(Some(14.0)).as_deref(),
            Some("idle")
        );
        assert!(locomotion.pending_corrections.is_empty());

        for tick in 0..40u32 {
            let state = if tick.is_multiple_of(2) {
                "idle"
            } else {
                "walk_forward"
            };
            locomotion.queue_correction(100 + tick, state);
        }
        assert_eq!(
            locomotion.pending_corrections.len(),
            MAX_PENDING_ANIMATION_CORRECTIONS
        );
        assert_eq!(
            locomotion
                .pending_corrections
                .front()
                .map(|pending| pending.server_tick),
            Some(124),
            "overflow drops the oldest"
        );
    }

    #[test]
    fn remote_player_correction_leading_forward_travel_keeps_legs_toward_travel() {
        // The host has reversed into a backpedal while the replicated velocity
        // still runs forward along aim (-Z).
        let (mut registry, mut client, id) = present_remote_player(
            remote_player_backpedal_mesh(),
            &["idle", "walk_forward", "walk_backward"],
            [0.0, 0.0, -60.0],
            (60.0, 60.0),
        );
        resolve_pending_animation_stamps(&mut registry, 1.0);
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                111,
                vec![delta(
                    7,
                    2,
                    3,
                    vec![mesh_animation_payload("walk_backward")],
                )],
            ),
        );
        client.sample_into_registry(&mut registry, 111.0, 1.0);
        let (state, heading) = presented_state_and_heading(&registry, &client, id);
        assert_eq!(state, "walk_backward", "the correction's clip holds");
        assert!(
            heading.abs() <= EPSILON,
            "legs face forward travel, not 180° behind aim: {heading}"
        );
    }

    #[test]
    fn remote_player_reversal_correction_holds_until_the_presented_travel_reverses() {
        let declared = ["idle", "walk_forward", "walk_backward"];
        let (mut registry, mut client, id) = present_remote_player(
            remote_player_backpedal_mesh(),
            &declared,
            [0.0, 0.0, -60.0],
            (60.0, 60.0),
        );
        // The host reverses into a backpedal before the replicated travel does,
        // and resends that state with every delta.
        resolve_pending_animation_stamps(&mut registry, 1.0);
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                111,
                vec![delta(
                    7,
                    2,
                    3,
                    vec![mesh_animation_payload("walk_backward")],
                )],
            ),
        );
        for anim_time in [1.0, 1.06, 1.2] {
            client.sample_into_registry(&mut registry, 111.0, anim_time);
            resolve_pending_animation_stamps(&mut registry, anim_time);
            let (state, heading) = presented_state_and_heading(&registry, &client, id);
            assert_eq!(
                state, "walk_backward",
                "no revert to the forward prediction at {anim_time}"
            );
            assert!(heading.abs() <= EPSILON, "legs follow forward travel");
        }
        // Once the presented travel reverses, the derivation agrees and the
        // correction releases with the legs folded toward aim.
        for (sequence, tick, baseline, next) in [(3, 120, 3, 4), (4, 130, 4, 5)] {
            client.apply_snapshot(
                &mut registry,
                &snapshot(
                    sequence,
                    tick,
                    vec![delta(
                        7,
                        baseline,
                        next,
                        vec![
                            transform_payload_at([0.0, 0.0, 0.0], 0.0),
                            movement_payload_with_velocity([0.0, 0.0, 60.0]),
                            mesh_animation_payload("walk_backward"),
                        ],
                    )],
                ),
            );
        }
        client.sample_into_registry(&mut registry, 125.0, 1.3);
        let (state, heading) = presented_state_and_heading(&registry, &client, id);
        assert_eq!(state, "walk_backward");
        assert!(heading.abs() <= EPSILON, "+Z travel folds to face aim");
        assert!(
            client.remote_player_locomotion[&NetworkId(7)]
                .authoritative_correction_state
                .is_none()
        );
    }

    #[test]
    fn remote_player_join_seeds_direction_from_the_host_state() {
        // A joiner meets a player strafing in the dead zone whose host state is
        // the backpedal; the first presented frame keeps that fold.
        let (mut registry, mut client, id) = map_remote_player(
            remote_player_backpedal_mesh(),
            &["idle", "walk_forward", "walk_backward"],
            [60.0, 0.0, 0.0],
            (60.0, 60.0),
        );
        assert!(apply_mesh_animation_state(
            &mut registry,
            id,
            "walk_backward",
            true
        ));
        client.seed_remote_player_direction(&mut registry, NetworkId(7), id, "walk_backward");
        client.sample_into_registry(&mut registry, 105.0, 1.0);
        let (state, heading) = presented_state_and_heading(&registry, &client, id);
        assert_eq!(state, "walk_backward");
        assert!((heading - std::f32::consts::FRAC_PI_2).abs() <= EPSILON);
    }

    #[test]
    fn remote_player_backward_correction_turns_legs_with_the_corrected_clip() {
        // +X travel is 90° off aim: inside the dead zone, so the client's fresh
        // classification stays forward and faces travel (yaw -π/2).
        let (mut registry, mut client, id) = present_remote_player(
            remote_player_backpedal_mesh(),
            &["idle", "walk_forward", "walk_backward"],
            [60.0, 0.0, 0.0],
            (60.0, 60.0),
        );
        let (state, heading) = presented_state_and_heading(&registry, &client, id);
        assert_eq!(state, "walk_forward");
        assert!((heading + std::f32::consts::FRAC_PI_2).abs() <= EPSILON);

        // The host, whose memory says backpedal, corrects to `walk_backward`.
        resolve_pending_animation_stamps(&mut registry, 1.0);
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                111,
                vec![delta(
                    7,
                    2,
                    3,
                    vec![mesh_animation_payload("walk_backward")],
                )],
            ),
        );
        client.sample_into_registry(&mut registry, 111.0, 1.0);
        let (state, heading) = presented_state_and_heading(&registry, &client, id);
        assert_eq!(state, "walk_backward");
        assert!(
            (heading - std::f32::consts::FRAC_PI_2).abs() <= EPSILON,
            "legs fold with the corrected backward clip: {heading}"
        );

        // After the crossfade the client's memory holds the backpedal in the band.
        resolve_pending_animation_stamps(&mut registry, 1.0);
        client.sample_into_registry(&mut registry, 111.0, 1.06);
        resolve_pending_animation_stamps(&mut registry, 1.06);
        client.sample_into_registry(&mut registry, 111.0, 1.06);
        let (state, heading) = presented_state_and_heading(&registry, &client, id);
        assert_eq!(state, "walk_backward");
        assert!((heading - std::f32::consts::FRAC_PI_2).abs() <= EPSILON);
    }

    // --- Starvation after sampling: a Transform-only remote (no velocity) holds its
    // last pose; the presented source flips to HeldNewest. ---
    #[test]
    fn transform_only_remote_holds_last_pose_after_starvation() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(7, 1, vec![transform_payload(4.0)])],
            ),
        );
        // Render tick far beyond the newest sample (110): the Transform-only mover has
        // no velocity, so the buffer holds the last pose.
        assert_eq!(
            client.presented_source(NetworkId(7), 200.0),
            Some(PoseSource::HeldNewest)
        );
        let stats = client.sample_into_registry(&mut registry, 200.0, 0.0);
        assert_eq!(stats.presented, 1, "one remote entity presented");
        assert_eq!(stats.held_newest, 1, "held-newest starvation is reported");
        assert_eq!(
            stats.starvation_feedback, 0,
            "one Transform-only sample is not enough evidence to raise global delay"
        );
        let id = *client.map().get(&NetworkId(7)).unwrap();
        let held = registry.interpolated_transform(id, 1.0).unwrap();
        assert!(
            (held.position.x - 4.0).abs() < EPSILON,
            "held the last pose"
        );
    }

    // Regression: acked unchanged stationary remotes are intentionally omitted by the
    // server, so their buffers can hold newest forever without indicating packet loss.
    #[test]
    fn stationary_remote_holding_newest_does_not_feed_starvation_delay() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![
                        transform_payload(4.0),
                        movement_payload_with_velocity([0.0, 0.0, 0.0]),
                    ],
                )],
            ),
        );
        client.apply_snapshot(&mut registry, &snapshot(1, 110, vec![]));

        let stats = client.sample_into_registry(&mut registry, 200.0, 0.0);
        assert_eq!(stats.presented, 1, "stationary remote still presents");
        assert_eq!(stats.held_newest, 1, "pose holds newest as expected");
        assert_eq!(
            stats.starvation_feedback, 0,
            "expected no-change hold must not raise the global delay"
        );
    }

    #[test]
    fn moving_remote_holding_newest_feeds_starvation_delay() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(
                    7,
                    1,
                    vec![transform_payload(4.0), movement_payload()],
                )],
            ),
        );

        let stats = client.sample_into_registry(&mut registry, 200.0, 0.0);
        assert_eq!(stats.presented, 1, "moving remote still presents");
        assert_eq!(stats.held_newest, 1, "pose holds newest after starvation");
        assert_eq!(
            stats.starvation_feedback, 1,
            "active remotes still raise delay when the buffer starves"
        );
    }

    // --- Despawn forgets the entity's interpolation buffer. ---
    #[test]
    fn despawn_forgets_interpolation_buffer() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                100,
                vec![full_baseline(7, 1, vec![transform_payload(0.0)])],
            ),
        );
        assert!(client.presented_source(NetworkId(7), 100.0).is_some());
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                110,
                vec![EntityRecord::Despawn {
                    network_id: 7,
                    tombstone_id: 1,
                    reason: 0,
                }],
            ),
        );
        assert!(
            client.presented_source(NetworkId(7), 100.0).is_none(),
            "despawn drops the buffer"
        );
    }

    // Regression: connected-client level unload clears registry entities while the
    // transport stays connected. The client must drop stale EntityId mappings and ask
    // the server for fresh baselines, or unchanged acked remotes can disappear forever.
    #[test]
    fn level_unload_reset_clears_mappings_and_requests_refresh() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                10,
                100,
                vec![full_baseline(7, 3, vec![transform_payload(0.0)])],
            ),
        );
        let old_entity = *client.map().get(&NetworkId(7)).expect("mapped");
        assert!(registry.exists(old_entity));
        assert!(client.presented_source(NetworkId(7), 100.0).is_some());

        let refreshes = client.reset_for_level_unload();

        assert!(client.map().is_empty(), "stale EntityId map is cleared");
        assert_eq!(
            client.stored_baseline(NetworkId(7)),
            None,
            "held baseline is cleared"
        );
        assert!(
            client.presented_source(NetworkId(7), 100.0).is_none(),
            "interpolation buffer is cleared"
        );
        assert!(
            client.is_pending_repair(NetworkId(7)),
            "old NetworkId remains pending so refreshes resend if needed"
        );
        assert_eq!(client.latest_sequence(), Some(10));
        assert_eq!(refreshes.len(), 1);
        assert_eq!(refreshes[0].snapshot_sequence, 10);
        assert_eq!(refreshes[0].network_id, 7);
        assert_eq!(refreshes[0].missing_baseline_ref, 3);
        assert_eq!(refreshes[0].reason, REFRESH_REASON_LEVEL_RELOAD);

        registry.clear_for_level_unload();
        let outcome = client.apply_snapshot(
            &mut registry,
            &snapshot(
                11,
                110,
                vec![full_baseline(7, 3, vec![transform_payload(2.0)])],
            ),
        );

        assert!(
            outcome.refresh_requests.is_empty(),
            "refresh response applies"
        );
        assert!(
            !client.is_pending_repair(NetworkId(7)),
            "fresh full baseline clears pending repair"
        );
        let new_entity = *client.map().get(&NetworkId(7)).expect("remapped");
        assert_ne!(
            new_entity, old_entity,
            "the NetworkId is remapped to a fresh level entity"
        );
        assert!(registry.exists(new_entity));
        assert_eq!(entity_pos(&registry, new_entity), Vec3::new(2.0, 0.0, 0.0));
    }

    // A local-player full baseline carries a movement payload (wire validation
    // requires `local_player: true` only on movement records).
    fn local_player_baseline(
        network_id: u32,
        baseline_id: u32,
        components: Vec<ComponentPayload>,
    ) -> EntityRecord {
        EntityRecord::FullBaseline {
            network_id,
            baseline_id,
            last_processed_client_tick: None,
            local_player: true,
            // A local movement pawn baseline names the descriptor class the host
            // materialized it from; the client materializes the matching component.
            entity_class: Some("player".to_string()),
            active_weapon_archetype: None,
            projectile_presentation: None,
            presentation_binding: None,
            components,
        }
    }

    fn local_player_delta(
        network_id: u32,
        baseline_ref: u32,
        new_baseline_id: u32,
        acked_tick: Option<u32>,
        components: Vec<ComponentPayload>,
    ) -> EntityRecord {
        EntityRecord::Delta {
            network_id,
            baseline_ref,
            new_baseline_id,
            last_processed_client_tick: acked_tick,
            local_player: true,
            entity_class: Some("player".to_string()),
            active_weapon_archetype: None,
            projectile_presentation: None,
            presentation_binding: None,
            components,
        }
    }

    // --- M15 Phase 3 Task 3: a `local_player` baseline marks the mapped pawn and
    // reports the armed (NetworkId, EntityId) for the caller to arm prediction. ---
    #[test]
    fn local_player_baseline_marks_pawn_and_reports_armed_pair() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![local_player_baseline(
                    7,
                    1,
                    vec![transform_payload(3.0), movement_payload()],
                )],
            ),
        );

        let id = *client.map().get(&NetworkId(7)).expect("local pawn mapped");
        // The mapped entity is marked as the local player pawn.
        assert_eq!(
            registry.local_player_pawn(),
            Some(id),
            "the local_player baseline marks the mapped pawn"
        );
        // The armed pair is reported for the caller to hand to ClientPrediction::arm,
        // carrying the descriptor class (Task 7) for client-side materialization.
        assert_eq!(
            out.armed_local_pawn,
            Some(ArmedLocalPawn {
                network_id: NetworkId(7),
                entity_id: id,
                entity_class: Some("player".to_string()),
            }),
            "apply reports the armed (NetworkId, EntityId, entity_class)"
        );
    }

    // Regression: once the local pawn is armed, apply_snapshot used to write the
    // authoritative Transform into the registry before reconcile ran. That erased the
    // pre-reconcile predicted pose, so smoothing measured the wrong correction and the
    // first-person camera snapped/rubber-banded instead of gliding.
    #[test]
    fn local_player_delta_preserves_predicted_registry_pose_for_reconcile() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![local_player_baseline(
                    7,
                    1,
                    vec![transform_payload(0.0), movement_payload()],
                )],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).expect("local pawn mapped");
        let predicted = Transform {
            position: Vec3::new(5.0, 0.0, 0.0),
            ..Transform::default()
        };
        registry
            .set_component(id, predicted)
            .expect("test seeds predicted pose");

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                12,
                vec![local_player_delta(
                    7,
                    1,
                    2,
                    Some(4),
                    vec![transform_payload(2.0), movement_payload()],
                )],
            ),
        );

        assert!(
            (entity_pos(&registry, id) - predicted.position).length() < EPSILON,
            "apply_snapshot must leave the armed local pawn's predicted pose for reconcile"
        );
        let reconcile = out
            .local_reconcile
            .expect("local authoritative record captured");
        assert!(
            (reconcile.transform.position - Vec3::new(2.0, 0.0, 0.0)).length() < EPSILON,
            "the authoritative pose is still surfaced to reconcile"
        );
        assert_eq!(
            reconcile.acked_tick,
            Some(4),
            "the host command ack is preserved for prune/replay"
        );
    }

    // The host refreshes the reconcile cursor every tick without bumping an unchanged
    // pawn's baseline, so while its ack is in flight it resends Delta(A->N) with a
    // newer `last_processed_client_tick`. That equal-id resend must still advance the
    // reconcile cursor, without re-acking or requesting repair.
    #[test]
    fn equal_id_local_delta_refreshes_reconcile_cursor_without_refresh_or_ack() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![local_player_baseline(
                    7,
                    1,
                    vec![transform_payload(0.0), movement_payload()],
                )],
            ),
        );
        let applied = client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                12,
                vec![local_player_delta(
                    7,
                    1,
                    2,
                    Some(4),
                    vec![transform_payload(2.0), movement_payload()],
                )],
            ),
        );
        assert_eq!(applied.ack.unwrap().entity_baselines, vec![(7, 2)]);

        let resend = client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                14,
                vec![local_player_delta(
                    7,
                    1,
                    2,
                    Some(6),
                    vec![transform_payload(2.0), movement_payload()],
                )],
            ),
        );

        let reconcile = resend
            .local_reconcile
            .expect("the metadata-only resend is captured for reconcile");
        assert_eq!(reconcile.acked_tick, Some(6), "the cursor advanced");
        assert_eq!(reconcile.server_tick, 14);
        assert!(
            (reconcile.transform.position - Vec3::new(2.0, 0.0, 0.0)).length() < EPSILON,
            "the resend carries the already-applied pose"
        );
        assert!(resend.refresh_requests.is_empty(), "no repair requested");
        assert!(!client.is_pending_repair(NetworkId(7)));
        assert!(
            resend.ack.unwrap().entity_baselines.is_empty(),
            "the held baseline is not re-acked"
        );
        assert_eq!(client.stored_baseline(NetworkId(7)), Some(2));
    }

    // Regression: the dev-tools "remote" capsule overlay used the raw client
    // NetworkId->EntityId map, which includes the local predicted pawn after a
    // local_player baseline. That drew a duplicate local capsule at the
    // prediction/reconcile seam and made it appear to vibrate slightly ahead of the
    // player. Remote markers must exclude the local pawn.
    #[test]
    fn remote_debug_entity_ids_excludes_local_predicted_pawn() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![
                    local_player_baseline(7, 1, vec![transform_payload(3.0), movement_payload()]),
                    full_baseline(8, 1, vec![transform_payload(9.0), movement_payload()]),
                ],
            ),
        );

        let local = *client.map().get(&NetworkId(7)).expect("local pawn mapped");
        let remote = *client.map().get(&NetworkId(8)).expect("remote pawn mapped");
        let ids: Vec<EntityId> = client.remote_debug_entity_ids().collect();

        assert_eq!(
            ids,
            vec![remote],
            "only non-local mapped entities are remote"
        );
        assert!(
            !ids.contains(&local),
            "the local predicted pawn must not be drawn as a remote debug capsule"
        );
    }

    // A non-local full baseline carrying a descriptor class (a remote entity).
    fn remote_entity_baseline(
        network_id: u32,
        baseline_id: u32,
        entity_class: &str,
        components: Vec<ComponentPayload>,
    ) -> EntityRecord {
        EntityRecord::FullBaseline {
            network_id,
            baseline_id,
            last_processed_client_tick: None,
            local_player: false,
            entity_class: Some(entity_class.to_string()),
            active_weapon_archetype: None,
            projectile_presentation: None,
            presentation_binding: None,
            components,
        }
    }

    fn remote_entity_delta(
        network_id: u32,
        baseline_ref: u32,
        new_baseline_id: u32,
        entity_class: &str,
        components: Vec<ComponentPayload>,
    ) -> EntityRecord {
        EntityRecord::Delta {
            network_id,
            baseline_ref,
            new_baseline_id,
            last_processed_client_tick: None,
            local_player: false,
            entity_class: Some(entity_class.to_string()),
            active_weapon_archetype: None,
            projectile_presentation: None,
            presentation_binding: None,
            components,
        }
    }

    // --- E10 Task 6: an unmapped, non-local full baseline carrying an `entity_class`
    // spawns Transform-only, maps the id (joins interpolation), and surfaces ONE
    // remote-entity materialize request carrying the mapped EntityId + class. ---
    #[test]
    fn non_local_class_bearing_baseline_surfaces_remote_entity_materialize() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![remote_entity_baseline(
                    7,
                    1,
                    "decraniated_mob",
                    vec![transform_payload(3.0)],
                )],
            ),
        );

        let id = *client
            .map()
            .get(&NetworkId(7))
            .expect("remote entity mapped");
        // The entity spawned Transform-only at the baseline pose and joined the
        // interpolation path (it is mapped, non-local).
        assert!((entity_pos(&registry, id).x - 3.0).abs() < EPSILON);
        // It is NOT marked the local pawn and reports no armed local pair.
        assert_eq!(registry.local_player_pawn(), None);
        assert!(out.armed_local_pawn.is_none());
        // Exactly one remote-entity materialize request, carrying the mapped id + class.
        assert_eq!(
            out.remote_entities,
            vec![RemoteEntityMaterialize {
                network_id: NetworkId(7),
                entity_id: id,
                entity_class: "decraniated_mob".to_string(),
                initial_animation_state: None,
                active_weapon_archetype: None,
                weapon_attachment_changed: false,
            }],
            "first spawn surfaces one remote-entity materialize request"
        );
    }

    #[test]
    fn remote_entity_spawn_surfaces_initial_mesh_animation_state() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![remote_entity_baseline(
                    7,
                    1,
                    "decraniated_mob",
                    vec![transform_payload(3.0), mesh_animation_payload("attack")],
                )],
            ),
        );

        let id = *client
            .map()
            .get(&NetworkId(7))
            .expect("remote entity mapped");
        assert_eq!(
            out.remote_entities,
            vec![RemoteEntityMaterialize {
                network_id: NetworkId(7),
                entity_id: id,
                entity_class: "decraniated_mob".to_string(),
                initial_animation_state: Some("attack".to_string()),
                active_weapon_archetype: None,
                weapon_attachment_changed: false,
            }],
            "the spawn request carries the baseline's initial mesh animation state"
        );
    }

    // --- E10 Task 6: a non-local baseline WITHOUT an entity_class surfaces no
    // materialize request (the Phase 2 dumb mover stays mesh-less). ---
    #[test]
    fn non_local_classless_baseline_surfaces_no_remote_entity() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![full_baseline(7, 1, vec![transform_payload(3.0)])],
            ),
        );

        assert!(
            client.map().contains_key(&NetworkId(7)),
            "still spawns + maps (interpolation)"
        );
        assert!(
            out.remote_entities.is_empty(),
            "a classless baseline surfaces no remote-entity materialize request"
        );
    }

    // --- Local player baselines bypass remote-entity materialization. ---
    #[test]
    fn local_player_baseline_surfaces_no_remote_entity() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![local_player_baseline(
                    7,
                    1,
                    vec![transform_payload(3.0), movement_payload()],
                )],
            ),
        );

        assert!(
            out.armed_local_pawn.is_some(),
            "the local pawn arms on the movement path"
        );
        assert!(
            out.remote_entities.is_empty(),
            "the local pawn never rides the remote-entity materialize path"
        );
    }

    #[test]
    fn movement_weapon_identity_surfaces_remote_and_local_attachment_updates() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let mut remote_baseline = remote_entity_baseline(
            7,
            1,
            "player",
            vec![transform_payload(0.0), movement_payload()],
        );
        let EntityRecord::FullBaseline {
            active_weapon_archetype,
            ..
        } = &mut remote_baseline
        else {
            unreachable!("fixture creates a baseline");
        };
        *active_weapon_archetype = Some("reference_pistol".to_string());

        let remote = client.apply_snapshot(&mut registry, &snapshot(0, 10, vec![remote_baseline]));
        let remote_id = *client.map().get(&NetworkId(7)).expect("remote pawn mapped");
        assert_eq!(
            remote.remote_entities,
            vec![RemoteEntityMaterialize {
                network_id: NetworkId(7),
                entity_id: remote_id,
                entity_class: "player".to_string(),
                initial_animation_state: None,
                active_weapon_archetype: Some("reference_pistol".to_string()),
                weapon_attachment_changed: true,
            }],
            "the initial shared weapon identity rides the descriptor-aware remote outcome"
        );

        let mut remote_delta = remote_entity_delta(
            7,
            1,
            2,
            "player",
            vec![transform_payload(1.0), movement_payload()],
        );
        let EntityRecord::Delta {
            active_weapon_archetype,
            ..
        } = &mut remote_delta
        else {
            unreachable!("fixture creates a delta");
        };
        *active_weapon_archetype = None;
        let unequipped = client.apply_snapshot(&mut registry, &snapshot(1, 11, vec![remote_delta]));
        assert_eq!(
            unequipped.remote_entities,
            vec![RemoteEntityMaterialize {
                network_id: NetworkId(7),
                entity_id: remote_id,
                entity_class: "player".to_string(),
                initial_animation_state: None,
                active_weapon_archetype: None,
                weapon_attachment_changed: true,
            }],
            "a change to None explicitly clears the remote hand attachment"
        );

        let mut local_baseline =
            local_player_baseline(8, 1, vec![transform_payload(2.0), movement_payload()]);
        let EntityRecord::FullBaseline {
            active_weapon_archetype,
            ..
        } = &mut local_baseline
        else {
            unreachable!("fixture creates a baseline");
        };
        *active_weapon_archetype = Some("reference_pistol".to_string());
        let local = client.apply_snapshot(&mut registry, &snapshot(2, 12, vec![local_baseline]));
        let local_id = *client.map().get(&NetworkId(8)).expect("local pawn mapped");
        assert_eq!(
            local.local_weapon_attachments,
            vec![LocalWeaponAttachmentUpdate {
                entity_id: local_id,
                active_weapon_archetype: Some("reference_pistol".to_string()),
            }],
            "the recipient-local shadow body receives the same third-person weapon update"
        );
        assert_eq!(
            client.local_active_weapon_archetype(),
            Some("reference_pistol"),
            "recipient-local viewmodel identity survives the arming record"
        );
        registry
            .set_component(local_id, MeshComponent::stateless("player".to_string()))
            .unwrap();

        let mut unchanged_local = delta(8, 1, 2, vec![movement_payload()]);
        let EntityRecord::Delta {
            local_player,
            active_weapon_archetype,
            ..
        } = &mut unchanged_local
        else {
            unreachable!("fixture creates a delta");
        };
        *local_player = true;
        *active_weapon_archetype = Some("reference_pistol".to_string());
        let unchanged =
            client.apply_snapshot(&mut registry, &snapshot(3, 13, vec![unchanged_local]));
        assert!(
            unchanged.local_weapon_attachments.is_empty(),
            "an unchanged applied record does not tear down and recreate presentation state"
        );
        assert_eq!(
            client.local_active_weapon_archetype(),
            Some("reference_pistol")
        );

        registry
            .remove_component::<MeshComponent>(local_id)
            .unwrap();
        let mut recovered_local = delta(8, 2, 3, vec![movement_payload()]);
        let EntityRecord::Delta {
            local_player,
            active_weapon_archetype,
            ..
        } = &mut recovered_local
        else {
            unreachable!();
        };
        *local_player = true;
        *active_weapon_archetype = Some("reference_pistol".to_string());
        let recovered_local =
            client.apply_snapshot(&mut registry, &snapshot(4, 14, vec![recovered_local]));
        assert_eq!(
            recovered_local.local_weapon_attachments,
            vec![LocalWeaponAttachmentUpdate {
                entity_id: local_id,
                active_weapon_archetype: Some("reference_pistol".to_string()),
            }],
            "a recovered local mesh reapplies cached active-weapon presentation"
        );

        client.apply_snapshot(
            &mut registry,
            &snapshot(
                5,
                15,
                vec![EntityRecord::Despawn {
                    network_id: 8,
                    tombstone_id: 9,
                    reason: 0,
                }],
            ),
        );
        assert_eq!(
            client.local_active_weapon_archetype(),
            None,
            "despawn clears recipient-local weapon presentation with its mapping"
        );
    }

    #[test]
    fn local_pawn_despawn_retires_current_inventory_and_preserves_transferred_items() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![local_player_baseline(
                    7,
                    1,
                    vec![transform_payload(0.0), movement_payload()],
                )],
            ),
        );
        let pawn = *client.map().get(&NetworkId(7)).unwrap();
        let first = registry.spawn(Transform::default());
        let second = registry.spawn(Transform::default());
        let mut inventory = postretro_entities::components::inventory::Inventory::default();
        inventory.wieldables[0] = Some(first);
        inventory.wieldables[3] = Some(second);
        registry.set_component(pawn, inventory.clone()).unwrap();
        assert_eq!(registry.inventory_owner(first), Some((pawn, 0)));
        assert_eq!(registry.inventory_owner(second), Some((pawn, 3)));
        // Retirement follows current membership, including a later acquisition.
        let other = registry.spawn(Transform::default());
        let later = registry.spawn(Transform::default());
        inventory.wieldables[3] = None;
        registry.set_component(pawn, inventory.clone()).unwrap();
        assert_eq!(registry.inventory_owner(second), None);
        let mut transferred = postretro_entities::components::inventory::Inventory::default();
        transferred.wieldables[8] = Some(second);
        registry.set_component(other, transferred).unwrap();
        inventory.wieldables[9] = Some(later);
        registry.set_component(pawn, inventory).unwrap();
        assert_eq!(registry.inventory_owner(second), Some((other, 8)));
        assert_eq!(registry.inventory_owner(later), Some((pawn, 9)));

        client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                11,
                vec![EntityRecord::Despawn {
                    network_id: 7,
                    tombstone_id: 2,
                    reason: 0,
                }],
            ),
        );

        assert!(!registry.exists(first));
        assert!(!registry.exists(later));
        assert!(!registry.exists(pawn));
        assert_eq!(registry.inventory_owner(first), None);
        assert_eq!(registry.inventory_owner(later), None);
        assert!(registry.exists(second));
        assert!(registry.exists(other));
        assert_eq!(registry.inventory_owner(second), Some((other, 8)));
        // Every recycled retired slot starts without inherited ownership.
        let retired = [pawn, first, later];
        for _ in retired {
            let fresh = registry.spawn(Transform::default());
            let prior = retired
                .iter()
                .find(|old| old.to_raw() & 0xffff == fresh.to_raw() & 0xffff)
                .unwrap();
            assert_ne!(fresh.to_raw() >> 16, prior.to_raw() >> 16);
            assert_eq!(registry.inventory_owner(fresh), None);
        }
    }

    // Regression: archetype identity can remain unchanged while the descriptor mesh
    // is recovered after a failed/removed presentation component. The new mesh still
    // needs its dynamic hand attachment and binding pass.
    #[test]
    fn recovered_player_mesh_reapplies_cached_weapon_attachment_identity() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let mut baseline = remote_entity_baseline(
            7,
            1,
            "player",
            vec![transform_payload(0.0), movement_payload()],
        );
        let EntityRecord::FullBaseline {
            active_weapon_archetype,
            ..
        } = &mut baseline
        else {
            unreachable!();
        };
        *active_weapon_archetype = Some("reference_pistol".to_string());
        let initial = client.apply_snapshot(&mut registry, &snapshot(0, 10, vec![baseline]));
        let pawn = initial.remote_entities[0].entity_id;
        registry
            .set_component(pawn, MeshComponent::stateless("player".to_string()))
            .unwrap();
        registry.remove_component::<MeshComponent>(pawn).unwrap();

        let mut unchanged = remote_entity_delta(
            7,
            1,
            2,
            "player",
            vec![transform_payload(1.0), movement_payload()],
        );
        let EntityRecord::Delta {
            active_weapon_archetype,
            ..
        } = &mut unchanged
        else {
            unreachable!();
        };
        *active_weapon_archetype = Some("reference_pistol".to_string());
        let recovered = client.apply_snapshot(&mut registry, &snapshot(1, 11, vec![unchanged]));

        assert_eq!(recovered.remote_entities.len(), 1);
        assert_eq!(
            recovered.remote_entities[0]
                .active_weapon_archetype
                .as_deref(),
            Some("reference_pistol")
        );
        assert!(recovered.remote_entities[0].weapon_attachment_changed);
    }

    #[test]
    fn initially_unarmed_local_mesh_materialization_surfaces_binding_resolution() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        let baseline =
            local_player_baseline(8, 1, vec![transform_payload(0.0), movement_payload()]);

        let outcome = client.apply_snapshot(&mut registry, &snapshot(0, 10, vec![baseline]));
        let pawn = outcome.armed_local_pawn.as_ref().unwrap().entity_id;
        assert_eq!(
            outcome.local_weapon_attachments,
            vec![LocalWeaponAttachmentUpdate {
                entity_id: pawn,
                active_weapon_archetype: None,
            }],
            "initial unarmed state still schedules the new mesh's binding pass"
        );
    }

    // --- E10 Task 6: a later delta and a re-baseline for the same NetworkId do NOT
    // re-surface a materialize request (no duplicate spawn, no reset of mesh state). ---
    #[test]
    fn remote_entity_delta_and_rebaseline_do_not_resurface_materialize() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        // First spawn surfaces one request.
        let spawn = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![remote_entity_baseline(
                    7,
                    1,
                    "decraniated_mob",
                    vec![transform_payload(0.0)],
                )],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).unwrap();
        assert_eq!(spawn.remote_entities.len(), 1);
        registry
            .set_component(id, MeshComponent::stateless("remote_enemy".to_string()))
            .expect("test marks remote as already materialized");
        client.cache_remote_presentation_complete(NetworkId(7), true);

        // A delta for the same (now mapped) id moves it but surfaces nothing.
        let delta_out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                11,
                vec![remote_entity_delta(
                    7,
                    1,
                    2,
                    "decraniated_mob",
                    vec![transform_payload(5.0)],
                )],
            ),
        );
        assert_eq!(
            *client.map().get(&NetworkId(7)).unwrap(),
            id,
            "delta mutates the same entity, no respawn"
        );
        assert!(
            delta_out.remote_entities.is_empty(),
            "a delta for a mapped remote entity surfaces no new materialize request"
        );

        // A re-baseline (mapped + live) for the same id also surfaces nothing.
        let rebaseline = client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                12,
                vec![remote_entity_baseline(
                    7,
                    9,
                    "decraniated_mob",
                    vec![transform_payload(8.0)],
                )],
            ),
        );
        assert_eq!(
            *client.map().get(&NetworkId(7)).unwrap(),
            id,
            "re-baseline updates in place, no respawn"
        );
        assert!(
            rebaseline.remote_entities.is_empty(),
            "a re-baseline for a mapped remote entity surfaces no new materialize request"
        );
    }

    // Regression: if a descriptor-class baseline was applied while remote
    // materialization could not attach a mesh, a later re-baseline must retry instead
    // of leaving the mapped entity transform-only forever.
    #[test]
    fn descriptor_presentation_completion_is_mesh_independent_and_clears_with_identity() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        let record = |baseline| {
            remote_entity_baseline(
                7,
                baseline,
                "invisible_target",
                vec![transform_payload(0.0)],
            )
        };
        let first = client.apply_snapshot(&mut registry, &snapshot(1, 10, vec![record(1)]));
        assert_eq!(first.remote_entities.len(), 1);
        let old = first.remote_entities[0].entity_id;
        // Even an attached mesh cannot stand in for a complete descriptor set.
        registry
            .set_component(old, MeshComponent::stateless("incomplete".into()))
            .unwrap();
        client.cache_remote_presentation_complete(NetworkId(7), false);
        let retry = client.apply_snapshot(&mut registry, &snapshot(2, 11, vec![record(2)]));
        assert_eq!(retry.remote_entities.len(), 1);
        registry.remove_component::<MeshComponent>(old).unwrap();
        client.cache_remote_presentation_complete(NetworkId(7), true);
        let complete = client.apply_snapshot(&mut registry, &snapshot(3, 12, vec![record(3)]));
        assert!(
            complete.remote_entities.is_empty(),
            "a complete meshless display copy does not reattach"
        );

        registry.despawn(old).unwrap();
        let stale = client.apply_snapshot(&mut registry, &snapshot(4, 13, vec![record(4)]));
        assert_eq!(stale.refresh_requests.len(), 1);
        assert!(!client.remote_presentation_complete.contains(&NetworkId(7)));
        let fresh = client.apply_snapshot(&mut registry, &snapshot(5, 14, vec![record(4)]));
        assert_eq!(fresh.remote_entities.len(), 1);
        assert_ne!(fresh.remote_entities[0].entity_id, old);
        client.cache_remote_presentation_complete(NetworkId(7), true);
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                6,
                15,
                vec![EntityRecord::Despawn {
                    network_id: 7,
                    tombstone_id: 1,
                    reason: 0,
                }],
            ),
        );
        assert!(client.remote_presentation_complete.is_empty());
        client.cache_remote_presentation_complete(NetworkId(99), true);
        assert!(
            client.remote_presentation_complete.is_empty(),
            "unmapped identity cannot populate cache"
        );
        for demote in [false, true] {
            client.apply_snapshot(
                &mut registry,
                &snapshot(7 + u32::from(demote), 16, vec![record(5)]),
            );
            client.cache_remote_presentation_complete(NetworkId(7), true);
            if demote {
                client.reset_for_demotion();
            } else {
                client.reset_for_level_unload();
            }
            assert!(client.remote_presentation_complete.is_empty());
        }
    }

    #[test]
    fn remote_entity_rebaseline_retries_when_mapped_entity_still_lacks_mesh() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let spawn = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![remote_entity_baseline(
                    7,
                    1,
                    "decraniated_mob",
                    vec![transform_payload(0.0)],
                )],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).unwrap();
        assert_eq!(spawn.remote_entities.len(), 1);
        assert_eq!(
            registry.has_component_kind(id, ComponentKind::Mesh),
            Ok(false),
            "test fixture leaves the first materialization unresolved"
        );

        let rebaseline = client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                11,
                vec![remote_entity_baseline(
                    7,
                    2,
                    "decraniated_mob",
                    vec![transform_payload(2.0), mesh_animation_payload("attack")],
                )],
            ),
        );

        assert_eq!(
            rebaseline.remote_entities,
            vec![RemoteEntityMaterialize {
                network_id: NetworkId(7),
                entity_id: id,
                entity_class: "decraniated_mob".to_string(),
                initial_animation_state: Some("attack".to_string()),
                active_weapon_archetype: None,
                weapon_attachment_changed: false,
            }],
            "mapped descriptor remotes with incomplete presentation retry materialization"
        );
    }

    // Regression: a baseline can materialize a descriptor mesh, then a later delta in
    // the same receive batch can arrive before clip indices resolve. The declared
    // state name must be staged instead of dropped.
    #[test]
    fn remote_entity_delta_applies_declared_animation_before_clips_resolve() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let spawn = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![remote_entity_baseline(
                    7,
                    1,
                    "decraniated_mob",
                    vec![transform_payload(0.0)],
                )],
            ),
        );
        let id = spawn.remote_entities[0].entity_id;
        registry
            .set_component(id, unresolved_mesh())
            .expect("test simulates descriptor materialization before clip resolve");

        client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                11,
                vec![remote_entity_delta(
                    7,
                    1,
                    2,
                    "decraniated_mob",
                    vec![mesh_animation_payload("attack")],
                )],
            ),
        );

        let mesh = registry
            .get_component::<MeshComponent>(id)
            .expect("remote mesh remains attached");
        assert_eq!(
            mesh.animation.as_ref().unwrap().current_state,
            "attack",
            "declared unresolved mesh-animation deltas are staged by name"
        );
    }

    #[test]
    fn unresolved_wire_state_entry_resets_prior_walk_playback_timeline() {
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        let mut mesh = unresolved_mesh();
        let animation = mesh.animation.as_mut().expect("animated mesh");
        animation.states.get_mut("locomotion").unwrap().clip_index = Some(0);
        animation.current_state = "locomotion".into();
        animation.entered_at = Some(0.0);
        animation.rebase_time = Some(0.0);
        animation.update_playback_rate(0.5, 0.0);
        registry.set_component(id, mesh).expect("attach mesh");

        assert!(apply_mesh_animation_state(
            &mut registry,
            id,
            "attack",
            true
        ));
        let staged = registry
            .get_component::<MeshComponent>(id)
            .expect("mesh")
            .animation
            .as_ref()
            .expect("animation");
        assert_eq!(staged.current_state, "attack");
        assert!((staged.rate - 1.0).abs() < EPSILON);
        assert_eq!(staged.rebase_time, None);
        assert_eq!(staged.rebase_elapsed, 0.0);

        let mut resolved = registry.get_component::<MeshComponent>(id).unwrap().clone();
        resolved
            .animation
            .as_mut()
            .unwrap()
            .states
            .get_mut("attack")
            .unwrap()
            .clip_index = Some(1);
        registry.set_component(id, resolved).unwrap();
        resolve_pending_animation_stamps(&mut registry, 4.0);
        let animation = registry
            .get_component::<MeshComponent>(id)
            .expect("mesh")
            .animation
            .as_ref()
            .expect("animation");
        assert!(
            (animation.scaled_elapsed(5.0) - 1.0).abs() < f64::EPSILON,
            "later clip resolution samples the newly entered state at its authored rate"
        );
    }

    // --- A non-local baseline never marks a pawn or reports an armed pair: before
    // the local_player baseline, prediction stays inert. ---
    #[test]
    fn non_local_baseline_does_not_mark_or_arm() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![full_baseline(
                    7,
                    1,
                    vec![transform_payload(3.0), movement_payload()],
                )],
            ),
        );

        assert_eq!(
            registry.local_player_pawn(),
            None,
            "a non-local baseline does not mark a local pawn"
        );
        assert!(
            out.armed_local_pawn.is_none(),
            "a non-local baseline reports no armed pair (prediction stays inert)"
        );
    }
}
