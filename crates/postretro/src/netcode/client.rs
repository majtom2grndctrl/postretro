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
// requests for presentation materialization, including retries while a mapped remote
// is still meshless. All registry mutation flows through the game-logic-owned apply
// primitives (`spawn`, `set_component_value`, `despawn`) — the net crate never
// touches the registry.
//
// State machine (per validated snapshot, applied in record order):
//   - FullBaseline, unmapped: spawn (Transform required), apply present payloads,
//     record the map + stored baseline, clear any pending repair, ack the baseline.
//   - FullBaseline, mapped + live: replace the stored baseline and update the
//     existing components in place (no respawn), clear pending repair, ack.
//   - FullBaseline, mapped + stale entity: drop the stale mapping, add to pending
//     repair, request a refresh. Not acked.
//   - Delta, baseline_ref held: apply, advance the stored baseline to
//     new_baseline_id, ack.
//   - Delta, baseline_ref unknown: add to pending repair, request a refresh, leave
//     state untouched. Not acked.
//   - Despawn: despawn the mapped entity (idempotent), drop the mapping, ack the
//     tombstone.
//   - Old/duplicate snapshot sequence: the whole snapshot is ignored.

use std::collections::{HashMap, VecDeque};

use glam::Vec3;

use postretro_net::wire::{
    AckMessage, BaselineRefreshRequest, COMPONENT_KIND_PLAYER_MOVEMENT_STATE, ClientMessage,
    ComponentPayload, EntityRecord, NetworkId, SnapshotMessage, WireGroundRef,
    WireKinematicMoverState, WirePlayerMovementState,
};

use postretro_entities::components::mesh::{
    FadeSourceKind, MeshComponent, SwitchResult, switch_animation_state,
};
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent,
    KinematicMoverMode, Transform,
};

use super::interpolation::{PoseSource, RemoteInterpolationBuffer, TransformSample};
use super::{payload_is_finite, wire_to_transform};
use crate::collision::moving::{MoverPose, MoverPoseSource};
use crate::kinematic_mover::{advance_mover_phase_one_tick, mover_pose_for_current_phase};

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
const REFRESH_REASON_UNKNOWN_BASELINE: u8 = 0;
/// Reason: a `FullBaseline` named a `NetworkId` whose mapped `EntityId` was stale.
const REFRESH_REASON_STALE_MAPPING: u8 = 1;
/// Reason: local level unload cleared registry-backed entities but kept the transport.
const REFRESH_REASON_LEVEL_RELOAD: u8 = 2;

/// Repair-request resend cadence: one `BaselineRefreshRequest` per pending entity
/// every 200 ms (5 Hz) until the matching full baseline arrives and clears it. The
/// reliable `Channel::Input` makes a single request sufficient in the common case;
/// the cadence covers the entity falling out of and back into the pending set.
const REPAIR_RESEND_INTERVAL_MS: f32 = 200.0;

/// Descriptor-immutable locomotion reference data for a remote enemy. The host
/// replicates only its changing pose and mesh state; the client reads this from the
/// shared descriptor table when that remote is materialized.
#[derive(Debug, Clone)]
struct RemoteEnemyWalkPlayback {
    move_speed: f32,
    walk_state: String,
}

/// A wire payload the client received but deliberately did not apply, recorded as a
/// typed diagnostic rather than silently dropped. Phase 2's dumb mover is
/// `Transform`-only; a `PlayerMovementState` payload on an unmapped full baseline
/// has no local descriptor-derived `PlayerMovementComponent` to merge onto, so it is
/// ignored (the substrate carries the wire type for later prediction).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IgnoredPayload {
    /// A `PlayerMovementState` payload arrived for an entity with no local
    /// `PlayerMovementComponent` to merge it onto. The `Transform` (if present) was
    /// still applied; only the movement subset was ignored.
    MovementWithoutLocalComponent { network_id: u32 },
    /// A mover payload referenced a PRL mover id this client did not load. This is
    /// an engine-side validation failure because the net crate is registry-blind.
    UnknownKinematicMover { network_id: u32, mover_id: u32 },
    /// A movement payload referenced a ground mover id this client did not load.
    /// The ground reference is engine-owned validation for the same reason.
    UnknownGroundMover { network_id: u32, mover_id: u32 },
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
pub(crate) struct ClientReplication {
    /// `NetworkId -> EntityId` for every entity this client has spawned from a full
    /// baseline and not yet despawned.
    map: HashMap<NetworkId, EntityId>,
    /// Reverse of `map`, maintained in lockstep so client-local hit records can name
    /// their target on the wire.
    reverse_map: HashMap<EntityId, NetworkId>,
    /// `NetworkId -> stored baseline_id`. The baseline the client currently holds for
    /// each mapped entity; a `Delta`'s `baseline_ref` must match this to apply, and a
    /// successful apply advances it. Kept in lockstep with `map`.
    baselines: HashMap<NetworkId, u32>,
    /// Entities awaiting a full-baseline refresh, keyed by `NetworkId`. An entry here
    /// resends a `BaselineRefreshRequest` on the 5 Hz cadence; the matching
    /// `FullBaseline` apply clears it.
    pending_repairs: HashMap<NetworkId, PendingRepair>,
    /// The highest snapshot sequence accepted so far. An older-or-equal sequence is a
    /// stale/duplicate packet and the whole snapshot is ignored. `None` until the
    /// first snapshot is accepted (sequence 0 is a valid first snapshot).
    latest_sequence: Option<u32>,
    /// The `server_tick` of the latest accepted snapshot — echoed back in the ack.
    acked_server_tick: u32,
    /// Per-remote-entity interpolation buffers keyed by `NetworkId` (Task 6). Each
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
    /// The local predicted pawn's `NetworkId`, once a `local_player` record has armed
    /// it (M15 Phase 3 Task 5). The local pawn is driven by client-side prediction +
    /// reconciliation, NOT the remote interpolation path: it is excluded from the
    /// interp buffer (`apply_components_to` skips recording it) and from
    /// `sample_into_registry`'s presentation writes (which would otherwise clobber the
    /// reconciled pose with a stale interpolated remote pose). `None` until armed.
    local_pawn: Option<NetworkId>,
    /// Network ids bound to PRL-loaded local kinematic movers, with their stable
    /// compile-time mover id. These entities are predicted/reconciled in place and
    /// must never be remote-interpolated or materialized from a baseline.
    mover_network_ids: HashMap<NetworkId, u32>,
    /// Authoritative mover samples from host snapshots, keyed by stable PRL
    /// `mover_id`. This is distinct from the live per-tick side-table; replay can
    /// read it through the `MoverPoseSource` seam.
    mover_history: MoverHistoryBuffer,
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
}

impl MoverHistoryBuffer {
    fn clear(&mut self) {
        self.samples.clear();
    }

    pub(crate) fn record(&mut self, mover_id: u32, sample: MoverHistorySample) {
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
/// snapshot, surfaced for the caller to materialize remote-enemy presentation (E10
/// Task 6). `ClientReplication` spawns the entity Transform-only and maps its
/// `NetworkId` (so it joins the remote interpolation path), but the descriptor tables
/// are not in scope here — the net-facing apply is descriptor-blind. The caller
/// (`client_receive_and_apply`, where the descriptor table is in scope) calls
/// `materialize_net_remote_enemy_presentation` to attach ONLY the descriptor's mesh.
///
/// Surfaced on the unmapped first-spawn of a non-local record carrying an
/// `entity_class`, and on later descriptor-class records only while the mapped entity
/// is still missing `Mesh`. The helper is idempotent, so retry repairs a
/// transform-only remote without resetting an already-materialized mesh. The local
/// predicted pawn is excluded: it has its own `armed_local_pawn` movement-path
/// materialization and is never a remote enemy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteEnemyMaterialize {
    /// Host-assigned identity for the mapped remote entity. The descriptor-aware
    /// receive glue uses this to cache its immutable locomotion reference data.
    pub(crate) network_id: NetworkId,
    /// The mapped `EntityId` the spawn produced (Transform-only at this point).
    pub(crate) entity_id: EntityId,
    /// The descriptor class the host stamped on the wire. The caller resolves this to
    /// a descriptor and attaches its mesh; an unregistered class leaves the entity
    /// transform-only (logged, not rejected).
    pub(crate) entity_class: String,
    /// Optional current mesh-animation state carried by the spawn baseline. It is
    /// applied after descriptor mesh materialization so a client joining an already
    /// active enemy does not miss the initial non-default animation state.
    pub(crate) initial_animation_state: Option<String>,
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
    /// The ack for this snapshot, or `None` if the snapshot was rejected (stale
    /// sequence). Carries only the applied baselines and tombstones —
    /// rejected/unknown-baseline records are never acked.
    pub(crate) ack: Option<AckMessage>,
    /// Refresh requests triggered by this snapshot (unknown-baseline deltas, stale
    /// mappings). Sent immediately; the pending set also resends them on cadence.
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
    /// Non-local, descriptor-class-bearing entities that need remote-enemy mesh
    /// presentation. Usually one entry per spawn; mapped re-baselines or deltas can
    /// surface another entry only while the entity is still meshless, so retry can
    /// repair a failed materialization without duplicating an already-live mesh. The
    /// descriptor lookup deliberately does NOT happen here — descriptor tables are not
    /// in scope in this descriptor-blind apply path.
    pub(crate) remote_enemies: Vec<RemoteEnemyMaterialize>,
    /// Per-snapshot mover correction magnitudes in metres, surfaced so harnesses
    /// can assert corrections stay bounded and non-accumulating.
    pub(crate) mover_corrections: Vec<MoverCorrection>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MoverCorrection {
    pub(crate) network_id: NetworkId,
    pub(crate) mover_id: u32,
    pub(crate) magnitude: f32,
}

impl ClientReplication {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Clear registry-backed replication state for a connected-client level unload
    /// while preserving connection-level snapshot ordering. Returns immediate refresh
    /// requests for every previously-known `NetworkId` so the server re-sends full
    /// baselines for unchanged, already-acked entities after the client registry is
    /// cleared.
    pub(crate) fn reset_for_level_unload(&mut self) -> Vec<BaselineRefreshRequest> {
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
        self.pending_repairs.clear();
        self.interp = RemoteInterpolationBuffer::default();
        self.remote_enemy_walk_playback.clear();
        self.local_pawn = None;
        self.mover_network_ids.clear();
        self.mover_history.clear();

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

    /// Read-only view of the current `NetworkId -> EntityId` map. Test-only; the
    /// dev-tools overlay uses `remote_debug_entity_ids` so it can exclude the local
    /// predicted pawn.
    #[cfg(test)]
    pub(crate) fn map(&self) -> &HashMap<NetworkId, EntityId> {
        &self.map
    }

    /// Resolve a client-local entity to the host-assigned `NetworkId`, if this
    /// entity is currently mapped by replication.
    #[allow(dead_code)]
    pub(crate) fn network_id_for_entity(&self, entity_id: EntityId) -> Option<NetworkId> {
        self.reverse_map.get(&entity_id).copied()
    }

    /// The host-assigned id of this client's owned pawn, once the `local_player`
    /// baseline has armed prediction.
    pub(crate) fn local_pawn_network_id(&self) -> Option<NetworkId> {
        self.local_pawn
    }

    /// Resolve a replicated `NetworkId` to the current client-local entity.
    #[allow(dead_code)]
    pub(crate) fn entity_for_network_id(&self, network_id: NetworkId) -> Option<EntityId> {
        self.map.get(&network_id).copied()
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

    /// Apply one validated snapshot. Rejects an old/duplicate sequence wholesale;
    /// otherwise walks the records in order, mutating the registry through the
    /// game-logic-owned primitives, and returns the ack + refresh requests + ignored
    /// diagnostics this snapshot produced.
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

    pub(crate) fn apply_snapshot_with_mover_target_tick(
        &mut self,
        registry: &mut EntityRegistry,
        snapshot: &SnapshotMessage,
        mover_target_tick: u32,
        mover_tick_dt: f32,
    ) -> ApplyOutcome {
        // Old or duplicate sequence: ignore the whole snapshot. The unreliable
        // snapshot channel can deliver an older packet after a newer one; applying it
        // would regress state. Sequence 0 is a valid first snapshot (None < Some(0)).
        if let Some(latest) = self.latest_sequence {
            if snapshot.sequence <= latest {
                return ApplyOutcome::default();
            }
        }
        self.latest_sequence = Some(snapshot.sequence);
        self.acked_server_tick = snapshot.server_tick;

        let mut outcome = ApplyOutcome::default();
        // Accumulate ackable progress; only applied records ack.
        let mut acked_baselines: Vec<(u32, u32)> = Vec::new();
        let mut acked_tombstones: Vec<(u32, u32)> = Vec::new();

        for record in &snapshot.records {
            match record {
                EntityRecord::FullBaseline {
                    network_id,
                    baseline_id,
                    components,
                    local_player,
                    last_processed_client_tick,
                    entity_class,
                } => {
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
                        acked_baselines.push((*network_id, *baseline_id));
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
                } => {
                    if self.apply_delta(
                        registry,
                        snapshot.sequence,
                        snapshot.server_tick,
                        mover_target_tick,
                        mover_tick_dt,
                        NetworkId(*network_id),
                        *baseline_ref,
                        *new_baseline_id,
                        components,
                        *local_player,
                        entity_class.as_deref(),
                        &mut outcome,
                    ) {
                        acked_baselines.push((*network_id, *new_baseline_id));
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
                EntityRecord::Despawn {
                    network_id,
                    tombstone_id,
                    ..
                } => {
                    self.apply_despawn(registry, NetworkId(*network_id));
                    // A despawn always acks its tombstone: the despawn is idempotent
                    // (unknown/already-gone is a no-op) and the client has, by the
                    // time it returns, reached the despawned state the tombstone
                    // names. Acking stops the server resending it.
                    acked_tombstones.push((*network_id, *tombstone_id));
                }
            }
        }

        // An ack is produced whenever the snapshot was accepted (advanced the
        // sequence) — even with no per-entity progress it carries the latest sequence
        // and server tick, which is the join-in-progress / keep-alive signal the
        // server reads. Refresh-only snapshots (no applied record) still ack the
        // sequence so the server's `last_acked_sequence` advances.
        outcome.ack = Some(AckMessage {
            latest_snapshot_sequence: snapshot.sequence,
            acked_server_tick: snapshot.server_tick,
            entity_baselines: acked_baselines,
            despawn_tombstones: acked_tombstones,
            // State-slot baselines are acked by the Task 3 client apply path; this
            // entity-apply ack carries none.
            slot_baselines: Vec::new(),
        });
        outcome
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
                self.maybe_surface_remote_enemy_materialize(
                    registry,
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
            // Drop the stale mapping, mark pending, and request a refresh. Leave all
            // other registry state untouched. Not acked.
            Some(_) => {
                self.remove_mapping(network_id);
                self.baselines.remove(&network_id);
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
                if let Some(mover_state) = first_mover_state(components) {
                    let Some(id) = find_loaded_mover_entity(registry, mover_state.mover_id) else {
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
                    self.mover_network_ids
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
                        self.mover_network_ids.remove(&network_id);
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
                // remote enemy. Surface a presentation-materialization request for the
                // caller (descriptor tables are not in scope here). Later mapped
                // descriptor records surface a retry only while the entity is still
                // meshless. The local pawn is excluded: its descriptor presentation
                // rides `armed_local_pawn` on the movement path, never the remote-enemy
                // mesh path.
                if !local_player {
                    if let Some(class) = entity_class {
                        outcome.remote_enemies.push(RemoteEnemyMaterialize {
                            network_id,
                            entity_id: id,
                            entity_class: class.to_string(),
                            initial_animation_state: first_mesh_animation_state(components),
                        });
                    }
                }
                true
            }
        }
    }

    /// Apply a `Delta`. Returns `true` if applied (ackable), `false` if it requested
    /// a refresh (unknown baseline ref). See the module state machine.
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
    ) -> bool {
        // The client must hold the referenced baseline and a live mapped entity. If
        // the stored baseline does not match (lost/old snapshot), or the entity is
        // gone, request a refresh and leave current state untouched.
        let held = self.baselines.get(&network_id).copied();
        let mapped = self.map.get(&network_id).copied();
        let appliable = matches!((held, mapped), (Some(b), Some(id))
            if b == baseline_ref && registry.exists(id));
        if !appliable {
            self.queue_repair(
                &mut outcome.refresh_requests,
                sequence,
                network_id,
                baseline_ref,
                REFRESH_REASON_UNKNOWN_BASELINE,
            );
            return false;
        }
        // Safe: `appliable` proved both are Some and the entity is live.
        let id = mapped.expect("appliable delta has a mapped entity");
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
            return false;
        }
        self.maybe_surface_remote_enemy_materialize(
            registry,
            id,
            local_player,
            entity_class,
            components,
            outcome,
        );
        // Advance the stored baseline so the next delta chains from this one. An
        // empty-component delta is a valid no-op apply: it still advances the baseline
        // (the server bumped the baseline id even if the mirrors did not change the
        // applied set), so the client stays in step.
        self.baselines.insert(network_id, new_baseline_id);
        true
    }

    fn maybe_surface_remote_enemy_materialize(
        &self,
        registry: &EntityRegistry,
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
        if registry
            .has_component_kind(entity_id, ComponentKind::Mesh)
            .unwrap_or(false)
        {
            return;
        }
        outcome.remote_enemies.push(RemoteEnemyMaterialize {
            network_id,
            entity_id,
            entity_class: class.to_string(),
            initial_animation_state: first_mesh_animation_state(components),
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
        self.interp.forget(network_id);
        self.remote_enemy_walk_playback.remove(&network_id);
        self.mover_network_ids.remove(&network_id);
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
            ComponentPayload::PlayerMovementState(m) => Some(*m),
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

    /// Despawn a mapped entity and drop its mapping + baseline. Idempotent: an unknown
    /// or already-despawned `NetworkId` is a no-op (the registry `despawn` of a stale
    /// id errors, which we swallow).
    fn apply_despawn(&mut self, registry: &mut EntityRegistry, network_id: NetworkId) {
        if let Some(id) = self.map.remove(&network_id) {
            self.reverse_map.remove(&id);
            // `despawn` errors on a stale id; the entity may already be gone. Either
            // way the post-state is "despawned", so the error is ignored.
            let _ = registry.despawn(id);
        }
        self.baselines.remove(&network_id);
        // A despawn also clears any pending repair for the entity: there is nothing
        // to repair once it is gone.
        self.pending_repairs.remove(&network_id);
        // Drop the entity's interpolation buffer; a later re-spawn under a fresh
        // NetworkId starts with an empty buffer.
        self.interp.forget(network_id);
        self.remote_enemy_walk_playback.remove(&network_id);
        self.mover_network_ids.remove(&network_id);
        // If the local predicted pawn despawned, forget it: prediction re-arms off a
        // future `local_player` baseline (a fresh NetworkId).
        if self.local_pawn == Some(network_id) {
            self.local_pawn = None;
        }
    }

    fn insert_mapping(&mut self, network_id: NetworkId, entity_id: EntityId) {
        if let Some(previous_entity) = self.map.insert(network_id, entity_id) {
            self.reverse_map.remove(&previous_entity);
        }
        if let Some(previous_network) = self.reverse_map.insert(entity_id, network_id) {
            self.map.remove(&previous_network);
        }
    }

    fn remove_mapping(&mut self, network_id: NetworkId) -> Option<EntityId> {
        let entity_id = self.map.remove(&network_id)?;
        self.reverse_map.remove(&entity_id);
        Some(entity_id)
    }

    /// Apply each component payload onto `id`. A `Transform` is written through
    /// `set_component_value` (idempotent — re-applying the spawn Transform is
    /// harmless and keeps this path uniform between spawn and update) and recorded
    /// into the per-entity interpolation buffer stamped by `server_tick`. A
    /// `PlayerMovementState` payload applies only to an entity that already carries a
    /// local `PlayerMovementComponent`; otherwise it is ignored with a typed
    /// diagnostic (Phase 2's dumb mover is Transform-only). Its `velocity` is still
    /// captured for the interpolation buffer's bounded extrapolation on starvation.
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
        // Capture the record's movement velocity (if any) up front: it stamps the
        // interpolation sample so a Transform-bearing record can extrapolate on
        // starvation. The Phase 2 dumb mover carries no movement payload, so this stays
        // None and its starvation path holds the last pose.
        let record_velocity = components.iter().find_map(|p| match p {
            ComponentPayload::PlayerMovementState(m) if payload_is_finite(p) => {
                Some(Vec3::from_array(m.velocity))
            }
            _ => None,
        });

        // The local predicted pawn is reconcile-driven: its authoritative pose +
        // movement subset are captured by `capture_local_reconcile` and the reconcile
        // path merges/replays them. Once armed, do NOT write its authoritative
        // Transform here: reconcile must still be able to read the pre-apply predicted
        // registry pose to seed the presentation offset. The arming snapshot is the
        // one exception (`local_pawn` is not set until `maybe_arm_local_pawn` runs
        // after this): it may seed the spawn/baseline pose here, and then `forget`
        // drops the one interpolation sample.
        let is_local = self.local_pawn == Some(network_id);
        let bound_mover_id = self.mover_network_ids.get(&network_id).copied();
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
                });
            }
            let _ = registry.set_component(id, plan.phase.clone());
            let _ = registry.set_component(id, plan.transform);
            self.mover_network_ids.insert(network_id, plan.mover_id);
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
                ComponentPayload::Transform(wire) => {
                    let transform = wire_to_transform(wire);
                    if !is_local {
                        if mover_apply.is_some()
                            || invalid_bound_mover_payload
                            || bound_mover_id.is_some()
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
                    {
                        self.interp.record(
                            network_id,
                            TransformSample {
                                server_tick,
                                transform,
                                velocity: record_velocity,
                            },
                        );
                    }
                }
                ComponentPayload::PlayerMovementState(_) => {
                    // Apply ONLY onto an entity that already has a descriptor-derived
                    // PlayerMovementComponent. The wire subset is not a full component
                    // and must never construct one (entity_model.md §7b: movement is
                    // descriptor-owned). Phase 2's mover has no local source, so this
                    // is ignored with a typed diagnostic. The local-merge path
                    // (descriptor-immutable params + this mutable subset) lands with
                    // prediction in Phase 3; there is no Phase 2 producer onto a
                    // movement entity, so there is no merge to perform yet.
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
                    let switched =
                        apply_mesh_animation_state(registry, id, &wire.current_state, true);
                    if !switched {
                        log::trace!(
                            "[Net] deferred mesh animation state `{}` for {network_id:?}",
                            wire.current_state
                        );
                    }
                }
                ComponentPayload::KinematicMoverState(wire) => {
                    if mover_apply.is_none() {
                        let _ = validate_kinematic_mover_state_binding(
                            registry,
                            id,
                            network_id,
                            bound_mover_id,
                            wire,
                        );
                    }
                }
            }
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
            if self.mover_network_ids.contains_key(&network_id) {
                continue;
            }
            let Some(pose) = self.interp.presented_pose(network_id, render_server_tick) else {
                continue; // no samples buffered yet
            };
            let _ = registry.set_presentation_transform(entity_id, pose.transform);
            stats.presented += 1;
            self.update_remote_enemy_walk_playback_rate(
                registry,
                network_id,
                entity_id,
                pose.speed_xz,
                frame_anim_time,
            );
            // Diagnostic: a HeldNewest after sustained starvation is the visible
            // freeze the buffer falls back to; logged sparingly at trace.
            if matches!(pose.source, PoseSource::HeldNewest) {
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

    /// Record (or clear) the immutable descriptor data used to derive a remote
    /// enemy's walk playback rate. Called by descriptor-aware receive glue, while
    /// all per-frame registry writes remain in [`Self::sample_into_registry`].
    pub(crate) fn cache_remote_enemy_walk_playback(
        &mut self,
        network_id: NetworkId,
        reference: Option<(f32, String)>,
    ) {
        match reference {
            Some((move_speed, walk_state)) => {
                self.remote_enemy_walk_playback.insert(
                    network_id,
                    RemoteEnemyWalkPlayback {
                        move_speed,
                        walk_state,
                    },
                );
            }
            None => {
                self.remote_enemy_walk_playback.remove(&network_id);
            }
        }
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
        let Ok(mut mesh) = registry.get_component::<MeshComponent>(entity_id).cloned() else {
            return;
        };
        let previous_mesh = mesh.clone();
        let Some(animation) = mesh.animation.as_mut() else {
            return;
        };
        let raw_ratio =
            if animation.current_state == reference.walk_state && reference.move_speed > 0.0 {
                speed_xz / reference.move_speed
            } else {
                1.0
            };
        animation.update_playback_rate(raw_ratio, frame_anim_time);
        // The animation helper's epsilon guard avoids rebasing on every frame.
        // Preserve that win by leaving the registry component untouched on a no-op.
        if mesh != previous_mesh {
            let _ = registry.set_component(entity_id, mesh);
        }
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
    ) -> Option<PoseSource> {
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

    #[cfg(test)]
    pub(crate) fn mover_history_sample_count(&self, mover_id: u32) -> usize {
        self.mover_history.sample_count(mover_id)
    }

    pub(crate) fn mover_history(&self) -> &MoverHistoryBuffer {
        &self.mover_history
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
        if find_loaded_mover_entity(registry, mover_id).is_none() {
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

fn find_loaded_mover_entity(registry: &EntityRegistry, mover_id: u32) -> Option<EntityId> {
    registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .find_map(|(id, value)| {
            let ComponentValue::KinematicMover(mover) = value else {
                return None;
            };
            (mover.mover_id == mover_id).then_some(id)
        })
}

#[derive(Debug, Clone)]
struct MoverApplyPlan {
    mover_id: u32,
    phase: KinematicMoverComponent,
    transform: Transform,
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
    let mut phase =
        validate_kinematic_mover_state_binding(registry, id, network_id, bound_mover_id, &wire)?;
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
        history_samples,
    })
}

fn validate_kinematic_mover_state_binding(
    registry: &EntityRegistry,
    id: EntityId,
    network_id: NetworkId,
    bound_mover_id: Option<u32>,
    wire: &WireKinematicMoverState,
) -> Option<KinematicMoverComponent> {
    let Ok(mover) = registry
        .get_component::<KinematicMoverComponent>(id)
        .cloned()
    else {
        log::warn!(
            "[Net] KinematicMoverState for mover_id {} applied to entity without KinematicMover",
            wire.mover_id
        );
        return None;
    };
    if let Some(bound) = bound_mover_id {
        if bound != wire.mover_id {
            log::warn!(
                "[Net] KinematicMoverState mover_id {} would rebind {network_id:?} from mover_id {}; dropping phase",
                wire.mover_id,
                bound
            );
            return None;
        }
    }
    if mover.mover_id != wire.mover_id {
        log::warn!(
            "[Net] KinematicMoverState mover_id {} does not match local mover_id {}; dropping phase",
            wire.mover_id,
            mover.mover_id
        );
        return None;
    }
    Some(mover)
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
    mover.current_linear_velocity = Vec3::from_array(wire.velocity);
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
    if !animation.states.contains_key(state) {
        return false;
    }
    animation.current_state = state.to_string();
    animation.entered_at = None;
    animation.previous_state = None;
    animation.previous_entered_at = None;
    animation.fade_source = FadeSourceKind::Clip;
    animation.interrupted_outgoing = None;
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

#[cfg(test)]
mod tests {
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

    fn movement_payload() -> ComponentPayload {
        movement_payload_with_velocity([1.0, 0.0, 0.0])
    }

    fn movement_payload_with_velocity(velocity: [f32; 3]) -> ComponentPayload {
        ComponentPayload::PlayerMovementState(WirePlayerMovementState {
            velocity,
            ground: WireGroundRef::World,
            air_jumps_remaining: 1,
            air_dashes_remaining: 1,
            dash_cooldown_ms: 0.0,
            air_ticks: 0,
            movement_state: WireMovementState::Normal,
            coyote_timer_ms: 0.0,
            jump_buffer_timer_ms: 0.0,
            jump_spent: false,
            capsule_half_height: 0.8,
            capsule_eye_height: 1.5,
        })
    }

    fn mesh_animation_payload(state: &str) -> ComponentPayload {
        ComponentPayload::MeshAnimationState(WireMeshAnimationState {
            current_state: state.to_string(),
        })
    }

    fn mover_payload(mover_id: u32) -> ComponentPayload {
        ComponentPayload::KinematicMoverState(WireKinematicMoverState {
            mover_id,
            segment_index: 1,
            direction: -1,
            mode: 1,
            segment_elapsed_ms: 25.0,
            wait_remaining_ms: 5.0,
            started: true,
            completed: false,
            velocity: [0.5, 0.0, 0.0],
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
            velocity: [1.0, 0.0, 0.0],
        })
    }

    fn spawn_loaded_mover(registry: &mut EntityRegistry, mover_id: u32) -> EntityId {
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                KinematicMoverComponent::new(
                    mover_id,
                    vec![Vec3::ZERO, Vec3::X],
                    1.0,
                    0.0,
                    KinematicMoverMode::PingPong,
                    true,
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
        assert_eq!(client.mover_network_ids.get(&NetworkId(7)), Some(&42));
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
    }

    // --- Unknown-baseline delta: not applied, pending repair set, refresh requested,
    // unrelated state untouched. ---
    #[test]
    fn delta_with_unknown_baseline_requests_refresh_and_leaves_state() {
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

        // A delta referencing baseline 5 (the client holds 1): unappliable.
        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(1, 2, vec![delta(7, 5, 6, vec![transform_payload(99.0)])]),
        );
        // State untouched: position unchanged, baseline still 1.
        assert!((entity_pos(&registry, id).x - 0.0).abs() < EPSILON);
        assert_eq!(client.stored_baseline(NetworkId(7)), Some(1));
        // Pending repair + a refresh request emitted, not acked.
        assert!(client.is_pending_repair(NetworkId(7)));
        assert_eq!(out.refresh_requests.len(), 1);
        let req = out.refresh_requests[0];
        assert_eq!(req.network_id, 7);
        assert_eq!(req.missing_baseline_ref, 5);
        // The ack carries the sequence advance but NO baseline for the unappliable
        // entity.
        let ack = out.ack.unwrap();
        assert!(
            ack.entity_baselines.is_empty(),
            "unknown-baseline delta not acked"
        );
    }

    // --- Empty delta is a no-op only when its baseline ref is known: it advances the
    // baseline (held ref) but errors-to-repair otherwise. ---
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

        // An empty delta whose ref is NOT held requests a refresh instead.
        let out2 = client.apply_snapshot(
            &mut registry,
            &snapshot(2, 3, vec![delta(7, 99, 100, vec![])]),
        );
        assert!(client.is_pending_repair(NetworkId(7)));
        assert_eq!(out2.refresh_requests.len(), 1);
        assert!(out2.ack.unwrap().entity_baselines.is_empty());
    }

    // --- Old / duplicate sequence: the whole snapshot is ignored. ---
    #[test]
    fn old_and_duplicate_sequences_are_ignored() {
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

        // A snapshot with an OLDER sequence (3 < 5): fully ignored, no mutation.
        let before = registry.exists(id);
        let mut count_before = client.map().len();
        let out_old = client.apply_snapshot(
            &mut registry,
            &snapshot(
                3,
                30,
                vec![full_baseline(8, 2, vec![transform_payload(7.0)])],
            ),
        );
        assert!(out_old.ack.is_none(), "ignored snapshot produces no ack");
        assert!(
            !client.map().contains_key(&NetworkId(8)),
            "old snapshot did not spawn"
        );
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

    // --- A refresh response (FullBaseline) clears the pending repair and re-maps. ---
    #[test]
    fn full_baseline_refresh_response_clears_pending_and_remaps() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        // Spawn, then receive an unknown-baseline delta to enter the pending set.
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                1,
                vec![full_baseline(7, 1, vec![transform_payload(0.0)])],
            ),
        );
        client.apply_snapshot(
            &mut registry,
            &snapshot(1, 2, vec![delta(7, 99, 100, vec![transform_payload(5.0)])]),
        );
        assert!(client.is_pending_repair(NetworkId(7)));

        // The refresh response arrives as a FullBaseline: applies in place, clears
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
        assert_eq!(client.stored_baseline(NetworkId(7)), Some(100));
        assert_eq!(out.ack.unwrap().entity_baselines, vec![(7, 100)]);
    }

    // --- Despawn: idempotent, drops mapping, acks tombstone; unknown despawn no-ops. ---
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
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                1,
                vec![full_baseline(7, 1, vec![transform_payload(0.0)])],
            ),
        );
        // Enter pending via an unknown-baseline delta.
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
        // Pre-map entity 9 so its despawn applies, and entity 10 at baseline 1.
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                1,
                vec![
                    full_baseline(9, 1, vec![transform_payload(0.0)]),
                    full_baseline(10, 2, vec![transform_payload(0.0)]),
                ],
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
                    // Does not apply: unknown baseline ref for entity 10 (holds 2).
                    delta(10, 99, 100, vec![transform_payload(9.0)]),
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
            client.is_pending_repair(NetworkId(10)),
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
            &snapshot(1, 2, vec![delta(7, 99, 100, vec![transform_payload(1.0)])]),
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
        client.cache_remote_enemy_walk_playback(NetworkId(7), Some((60.0, "locomotion".into())));

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
        client.cache_remote_enemy_walk_playback(NetworkId(7), Some((60.0, "locomotion".into())));

        let before = registry
            .get_component::<MeshComponent>(id)
            .expect("mesh")
            .clone();
        client.sample_into_registry(&mut registry, 105.0, 1.0);
        let after = registry.get_component::<MeshComponent>(id).expect("mesh");
        assert_eq!(after, &before, "non-walk state must not rebase or write");
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

    // A non-local full baseline carrying a descriptor class (a remote enemy).
    fn remote_enemy_baseline(
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
            components,
        }
    }

    fn remote_enemy_delta(
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
            components,
        }
    }

    // --- E10 Task 6: an unmapped, non-local full baseline carrying an `entity_class`
    // spawns Transform-only, maps the id (joins interpolation), and surfaces ONE
    // remote-enemy materialize request carrying the mapped EntityId + class. ---
    #[test]
    fn non_local_class_bearing_baseline_surfaces_remote_enemy_materialize() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![remote_enemy_baseline(
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
            .expect("remote enemy mapped");
        // The entity spawned Transform-only at the baseline pose and joined the
        // interpolation path (it is mapped, non-local).
        assert!((entity_pos(&registry, id).x - 3.0).abs() < EPSILON);
        // It is NOT marked the local pawn and reports no armed local pair.
        assert_eq!(registry.local_player_pawn(), None);
        assert!(out.armed_local_pawn.is_none());
        // Exactly one remote-enemy materialize request, carrying the mapped id + class.
        assert_eq!(
            out.remote_enemies,
            vec![RemoteEnemyMaterialize {
                network_id: NetworkId(7),
                entity_id: id,
                entity_class: "decraniated_mob".to_string(),
                initial_animation_state: None,
            }],
            "first spawn surfaces one remote-enemy materialize request"
        );
    }

    #[test]
    fn remote_enemy_spawn_surfaces_initial_mesh_animation_state() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![remote_enemy_baseline(
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
            .expect("remote enemy mapped");
        assert_eq!(
            out.remote_enemies,
            vec![RemoteEnemyMaterialize {
                network_id: NetworkId(7),
                entity_id: id,
                entity_class: "decraniated_mob".to_string(),
                initial_animation_state: Some("attack".to_string()),
            }],
            "the spawn request carries the baseline's initial mesh animation state"
        );
    }

    // --- E10 Task 6: a non-local baseline WITHOUT an entity_class surfaces no
    // materialize request (the Phase 2 dumb mover stays mesh-less). ---
    #[test]
    fn non_local_classless_baseline_surfaces_no_remote_enemy() {
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
            out.remote_enemies.is_empty(),
            "a classless baseline surfaces no remote-enemy materialize request"
        );
    }

    // --- E10 Task 6: the local pawn is excluded from the remote-enemy path even if a
    // class rides its baseline — its descriptor presentation rides `armed_local_pawn`. ---
    #[test]
    fn local_player_baseline_surfaces_no_remote_enemy() {
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
            out.remote_enemies.is_empty(),
            "the local pawn never rides the remote-enemy materialize path"
        );
    }

    // --- E10 Task 6: a later delta and a re-baseline for the same NetworkId do NOT
    // re-surface a materialize request (no duplicate spawn, no reset of mesh state). ---
    #[test]
    fn remote_enemy_delta_and_rebaseline_do_not_resurface_materialize() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        // First spawn surfaces one request.
        let spawn = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![remote_enemy_baseline(
                    7,
                    1,
                    "decraniated_mob",
                    vec![transform_payload(0.0)],
                )],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).unwrap();
        assert_eq!(spawn.remote_enemies.len(), 1);
        registry
            .set_component(id, MeshComponent::stateless("remote_enemy".to_string()))
            .expect("test marks remote as already materialized");

        // A delta for the same (now mapped) id moves it but surfaces nothing.
        let delta_out = client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                11,
                vec![remote_enemy_delta(
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
            delta_out.remote_enemies.is_empty(),
            "a delta for a mapped remote enemy surfaces no new materialize request"
        );

        // A re-baseline (mapped + live) for the same id also surfaces nothing.
        let rebaseline = client.apply_snapshot(
            &mut registry,
            &snapshot(
                2,
                12,
                vec![remote_enemy_baseline(
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
            rebaseline.remote_enemies.is_empty(),
            "a re-baseline for a mapped remote enemy surfaces no new materialize request"
        );
    }

    // Regression: if a descriptor-class baseline was applied while remote
    // materialization could not attach a mesh, a later re-baseline must retry instead
    // of leaving the mapped entity transform-only forever.
    #[test]
    fn remote_enemy_rebaseline_retries_when_mapped_entity_still_lacks_mesh() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let spawn = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![remote_enemy_baseline(
                    7,
                    1,
                    "decraniated_mob",
                    vec![transform_payload(0.0)],
                )],
            ),
        );
        let id = *client.map().get(&NetworkId(7)).unwrap();
        assert_eq!(spawn.remote_enemies.len(), 1);
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
                vec![remote_enemy_baseline(
                    7,
                    2,
                    "decraniated_mob",
                    vec![transform_payload(2.0), mesh_animation_payload("attack")],
                )],
            ),
        );

        assert_eq!(
            rebaseline.remote_enemies,
            vec![RemoteEnemyMaterialize {
                network_id: NetworkId(7),
                entity_id: id,
                entity_class: "decraniated_mob".to_string(),
                initial_animation_state: Some("attack".to_string()),
            }],
            "mapped descriptor remotes without Mesh retry materialization"
        );
    }

    // Regression: a baseline can materialize a descriptor mesh, then a later delta in
    // the same receive batch can arrive before clip indices resolve. The declared
    // state name must be staged instead of dropped.
    #[test]
    fn remote_enemy_delta_applies_declared_animation_before_clips_resolve() {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();

        let spawn = client.apply_snapshot(
            &mut registry,
            &snapshot(
                0,
                10,
                vec![remote_enemy_baseline(
                    7,
                    1,
                    "decraniated_mob",
                    vec![transform_payload(0.0)],
                )],
            ),
        );
        let id = spawn.remote_enemies[0].entity_id;
        registry
            .set_component(id, unresolved_mesh())
            .expect("test simulates descriptor materialization before clip resolve");

        client.apply_snapshot(
            &mut registry,
            &snapshot(
                1,
                11,
                vec![remote_enemy_delta(
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
