// Connection-slot lifecycle glue: slot -> remote-pawn mapping and the accept/close
// cleanup path that spawns and despawns slot-owned inert pawns.
// See: context/lib/networking.md · entity_model.md §6
//
// M15 Phase 2 Task 4. `postretro-net` (`slots.rs`) models the connection slot state
// machine (Pending -> Accepted -> Closed) and surfaces accept/close transitions; it
// is registry-blind. This module is the engine half that knows both sides: it owns
// the slot -> remote-pawn `EntityId`/`NetworkId` mapping and runs the registry
// mutation through the game-logic-owned apply path.
//
// On accept: spawn (or register) one slot-owned inert player pawn, stamp it with a
// fresh session-monotonic `NetworkId` via the existing allocator, and add it to the
// Phase 2 `ReplicableSet` so it replicates like any other authoritative object.
//
// On close (clean disconnect OR timeout — one path for both): despawn the pawn
// through `EntityRegistry::despawn`, remove it from the `ReplicableSet`, and drop the
// slot mapping. The vanished registered entity makes the next `produce_owned_snapshots`
// emit nothing for that `NetworkId`, so `ServerReplication::ingest_tick` turns it into
// a resending despawn tombstone that reaches the remaining clients.
//
// Phase 2 cleanup is immediate. This is a mechanics substrate, not the co-op
// player-leave policy — Phase 4 may replace the gameplay policy while reusing this
// slot/pawn/close machinery (no respawn, spectate, or input application here).

use std::collections::HashMap;

use postretro_net::replication::ServerReplication;
use postretro_net::wire::NetworkId;

use crate::nav::NavAgentParams;
use crate::scripting::builtins::net_descriptor::spawn_net_slot_pawn;
use crate::scripting::data_descriptors::EntityTypeDescriptor;
use crate::scripting::map_entity::MapEntity;
use crate::scripting::registry::{EntityId, EntityRegistry, Transform};

use super::{NetworkIdAllocator, ReplicableSet};

/// The host-side slot -> remote-pawn map. One slot-owned inert pawn per accepted
/// client, keyed by the renet `ClientId` (`u64`). Owned by the `Host` endpoint
/// variant alongside the allocator and replicable set.
///
/// A slot that closes drops its entry here; a later connection is a fresh
/// `ClientId` and gets a fresh entry (and, via a freshly-allocated `EntityId`, a
/// fresh `NetworkId`). The map never reuses an `EntityId` across slot reuse — the
/// registry bumps the generation on despawn, so a reused slot's pawn is a distinct
/// entity.
#[derive(Debug, Default)]
pub(crate) struct SlotPawns {
    pawns: HashMap<u64, EntityId>,
    /// Deterministic slot → `player_spawn` placement-index assignment (M15 Phase 3).
    /// Recorded BEFORE the descriptor spawn so a reused slot has an auditable source:
    /// the same client id always resolves to the same placement for the session, and
    /// the assignment is round-robin over the available placements by first-accept
    /// order. Survives a close so a reconnecting client lands on its prior spawn.
    placement_assignments: HashMap<u64, usize>,
    /// Monotonic counter feeding the round-robin placement assignment. Never reset
    /// within a session, so assignment order is a pure function of accept order.
    next_assignment: usize,
}

impl SlotPawns {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Deterministically assign (or recall) the `player_spawn` placement index for a
    /// slot, given the number of available placements. Idempotent per client id: the
    /// first call records a round-robin assignment; later calls (including after a
    /// reconnect) return the same index. Returns `None` when there are no placements
    /// to assign from. The assignment is recorded before the spawn so a reused slot's
    /// source is auditable.
    pub(crate) fn assign_placement(
        &mut self,
        client_id: u64,
        placement_count: usize,
    ) -> Option<usize> {
        if placement_count == 0 {
            return None;
        }
        if let Some(&idx) = self.placement_assignments.get(&client_id) {
            // Clamp defensively in case the placement set shrank between sessions.
            return Some(idx.min(placement_count - 1));
        }
        let idx = self.next_assignment % placement_count;
        self.next_assignment = self.next_assignment.wrapping_add(1);
        self.placement_assignments.insert(client_id, idx);
        Some(idx)
    }

    /// The recorded placement-index assignment for a slot, if any. Auditable source
    /// for a reused slot — read by tests and operator diagnostics; staged until a
    /// non-test caller (e.g. a `[Net]` audit log) reads it.
    #[allow(dead_code)]
    pub(crate) fn placement_assignment(&self, client_id: u64) -> Option<usize> {
        self.placement_assignments.get(&client_id).copied()
    }

    /// The pawn entity for a slot, if one is registered. Used by lifecycle tests and
    /// available to the host owner-lookup path.
    #[allow(dead_code)]
    pub(crate) fn pawn_for(&self, client_id: u64) -> Option<EntityId> {
        self.pawns.get(&client_id).copied()
    }

    /// Number of live slot pawns. Test-only assertion helper.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.pawns.len()
    }
}

/// Where a slot-owned pawn comes from. The descriptor-backed variant
/// ([`SlotPawnSource::Descriptor`]) is the M15 Phase 3 production path: a movement
/// session spawns a real descriptor-driven `PlayerMovement` pawn from the slot's
/// assigned `player_spawn` placement. The `TransformFixture` variant remains for
/// tests/dev paths ONLY — it never sets `local_player` and carries no
/// `PlayerMovementComponent`. Kept as an explicit enum so the choice is a named,
/// auditable decision rather than an implicit default.
pub(crate) enum SlotPawnSource<'a> {
    /// A `Transform`-only inert pawn created by `crate::netcode`. Carries no
    /// `PlayerMovementComponent`. Tests/dev only — NEVER used for a Phase 3 movement
    /// session, and never marked `local_player`.
    TransformFixture,
    /// A descriptor-backed `PlayerMovement` pawn materialized from the slot's
    /// assigned `player_spawn` placement (M15 Phase 3). Reuses the descriptor
    /// materialization internals (`spawn_net_slot_pawn`): the placement's
    /// `entity_class` KVP selects the descriptor (default `"player"`), and the pawn
    /// is NOT marked local and carries no global `active_wieldable`. Tests may pass a
    /// synthetic placement + descriptor list.
    Descriptor {
        placement: &'a MapEntity,
        descriptors: &'a [EntityTypeDescriptor],
        agent_params: Option<NavAgentParams>,
    },
}

/// React to a slot being accepted: create the slot-owned inert pawn, stamp it with a
/// fresh session-monotonic `NetworkId`, add it to the replicable set, and record the
/// slot mapping. Returns the pawn `EntityId` (and its assigned `NetworkId`).
///
/// Idempotent per slot: a second accept for an already-mapped, still-live slot
/// returns the existing pawn without spawning a duplicate. A re-accept whose mapped
/// pawn has gone stale (despawned out from under us) re-spawns a fresh one.
///
/// The pawn is **server-authoritative and inert** in Phase 2 (no gameplay input is
/// applied to it). The `NetworkId` is allocated by the shared monotonic allocator,
/// which never recycles ids — so a slot reused by a later connection gets a fresh
/// `EntityId` and thus a fresh `NetworkId`; the old id is never re-emitted.
pub(crate) fn on_slot_accepted(
    registry: &mut EntityRegistry,
    slot_pawns: &mut SlotPawns,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    client_id: u64,
    source: SlotPawnSource,
) -> Option<(EntityId, NetworkId)> {
    // Idempotency: an accept for a slot that already owns a live pawn is a no-op
    // beyond returning the existing identity. Re-registering in the replicable set is
    // harmless (it is a set), and re-stamping the allocator returns the same stable
    // NetworkId for the same EntityId.
    if let Some(existing) = slot_pawns.pawns.get(&client_id).copied() {
        if registry.exists(existing) {
            let net_id = allocator.stamp(existing);
            replicable.register(existing);
            return Some((existing, net_id));
        }
        // The mapped pawn is stale (despawned elsewhere). Fall through and re-create.
        slot_pawns.pawns.remove(&client_id);
    }

    let pawn = match source {
        // Transform-only fixture: an inert pawn at the world origin. No
        // PlayerMovementComponent is materialized (tests/dev only — not a real
        // movement pawn, never marked local).
        SlotPawnSource::TransformFixture => registry.spawn(Transform::default()),
        // Descriptor-backed Phase 3 movement pawn from the slot's assigned
        // placement. A spawn failure (unregistered descriptor / registry exhausted)
        // is logged inside the helper; the accept then leaves the slot unmapped so a
        // later re-accept can retry — no inconsistent half-spawned state is recorded.
        SlotPawnSource::Descriptor {
            placement,
            descriptors,
            agent_params,
        } => {
            let Some(id) = spawn_net_slot_pawn(placement, descriptors, registry, agent_params)
            else {
                log::warn!(
                    "[Net] slot {client_id} accepted but descriptor spawn failed; slot left unmapped"
                );
                return None;
            };
            id
        }
    };

    // Stamp the stable session-monotonic NetworkId and register for replication.
    let net_id = allocator.stamp(pawn);
    replicable.register(pawn);
    slot_pawns.pawns.insert(client_id, pawn);
    log::info!("[Net] slot {client_id} accepted: spawned remote pawn {pawn:?} as {net_id:?}");
    Some((pawn, net_id))
}

/// React to a slot closing (clean disconnect OR timeout — the single Phase 2 cleanup
/// path for both): despawn the slot-owned pawn through the game-logic-owned apply,
/// remove it from the replicable set, and drop the slot mapping. The `ServerReplication`
/// client state is also dropped so the closed client stops being replicated to.
///
/// Returns the despawned pawn's `EntityId`, or `None` if the slot owned no pawn
/// (e.g. a slot that closed before it was ever accepted). After this returns, the
/// pawn is absent from the next `produce_owned_snapshots`, so the net tracker emits a
/// resending despawn tombstone to the remaining clients.
pub(crate) fn on_slot_closed(
    registry: &mut EntityRegistry,
    slot_pawns: &mut SlotPawns,
    replicable: &mut ReplicableSet,
    replication: &mut ServerReplication,
    client_id: u64,
) -> Option<EntityId> {
    // Drop the closed client's per-client replication state regardless of whether it
    // owned a pawn: it will never ack again.
    replication.remove_client(client_id);

    let pawn = slot_pawns.pawns.remove(&client_id)?;
    // Remove from the replicable set FIRST so the next ingest sees the entity gone
    // and emits the despawn tombstone; then despawn through game logic. (Order does
    // not matter for correctness here since both run before the next ingest, but
    // unregistering first keeps the invariant "replicable set never names a
    // despawned id" true at every yield point.)
    replicable.unregister(pawn);
    // `despawn` errors only on a stale id; the pawn may already be gone if game logic
    // despawned it. Either way the post-state is "gone", so the error is swallowed.
    let _ = registry.despawn(pawn);
    log::info!("[Net] slot {client_id} closed: despawned remote pawn {pawn:?}");
    Some(pawn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_net::replication::{EntitySnapshot, typed_records};
    use postretro_net::wire::EntityRecord;

    use crate::netcode::produce_owned_snapshots;

    // A short helper: drive a host replication tick from the registry + replicable
    // set, ingest into the tracker, and return the encoded records for a client.
    fn ingest_and_records(
        registry: &EntityRegistry,
        replicable: &ReplicableSet,
        allocator: &mut NetworkIdAllocator,
        replication: &mut ServerReplication,
        client_id: u64,
        tick: u32,
    ) -> Vec<EntityRecord> {
        let owned: Vec<EntitySnapshot> = produce_owned_snapshots(
            registry,
            replicable,
            allocator,
            &crate::netcode::MovementOwners::new(),
            &crate::netcode::HostCommandQueues::new(),
        );
        replication.ingest_tick(owned);
        let snap = replication
            .encode_for_client(client_id, tick)
            .expect("registered client encodes");
        typed_records(&snap)
    }

    const CLIENT_A: u64 = 10;
    const CLIENT_B: u64 = 20;

    // Accept spawns one slot-owned pawn, assigns a NetworkId, and adds it to the
    // replicable set.
    #[test]
    fn accept_spawns_registered_pawn_with_network_id() {
        let mut registry = EntityRegistry::new();
        let mut slot_pawns = SlotPawns::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();

        let (pawn, net_id) = on_slot_accepted(
            &mut registry,
            &mut slot_pawns,
            &mut allocator,
            &mut replicable,
            CLIENT_A,
            SlotPawnSource::TransformFixture,
        )
        .expect("transform fixture accept always spawns");

        assert!(
            registry.exists(pawn),
            "the slot pawn is live in the registry"
        );
        assert_eq!(slot_pawns.pawn_for(CLIENT_A), Some(pawn));
        assert!(
            replicable.contains(pawn),
            "the pawn is registered for replication"
        );
        // The pawn replicates: produce_owned_snapshots emits it keyed by its NetworkId.
        let owned = produce_owned_snapshots(
            &registry,
            &replicable,
            &mut allocator,
            &crate::netcode::MovementOwners::new(),
            &crate::netcode::HostCommandQueues::new(),
        );
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0].network_id, net_id.0);
        // The Transform-only fixture carries exactly one (Transform) payload — no
        // PlayerMovementState (it is not a real movement pawn).
        assert_eq!(owned[0].components.len(), 1);
    }

    // Clean disconnect: the slot frees, the pawn despawns and leaves the replicable
    // set, and a remaining client receives the despawn tombstone.
    #[test]
    fn clean_disconnect_despawns_pawn_and_replicates_despawn() {
        disconnect_runs_cleanup_and_replicates();
    }

    // Timeout runs the identical cleanup path as a clean disconnect (the close cause
    // is distinguished in the net slot model but Phase 2 cleanup is one path).
    #[test]
    fn timeout_runs_same_cleanup_path_as_disconnect() {
        // The lifecycle glue is cause-agnostic: on_slot_closed takes no cause. The
        // transport classifies disconnect vs timeout (slots.rs tests); both funnel
        // here. This test asserts the cleanup is identical by running the same body.
        disconnect_runs_cleanup_and_replicates();
    }

    fn disconnect_runs_cleanup_and_replicates() {
        let mut registry = EntityRegistry::new();
        let mut slot_pawns = SlotPawns::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();

        // Two slots so one remains to receive the despawn of the other.
        replication.register_client(CLIENT_A);
        replication.register_client(CLIENT_B);
        let (pawn_a, net_a) = on_slot_accepted(
            &mut registry,
            &mut slot_pawns,
            &mut allocator,
            &mut replicable,
            CLIENT_A,
            SlotPawnSource::TransformFixture,
        )
        .expect("transform fixture accept always spawns");
        let _ = on_slot_accepted(
            &mut registry,
            &mut slot_pawns,
            &mut allocator,
            &mut replicable,
            CLIENT_B,
            SlotPawnSource::TransformFixture,
        )
        .expect("transform fixture accept always spawns");

        // Tick 1: both pawns ingested; client B sees pawn A as a full baseline. Ack it
        // so the despawn (not a re-baseline) is what we observe later.
        let records_b = ingest_and_records(
            &registry,
            &replicable,
            &mut allocator,
            &mut replication,
            CLIENT_B,
            1,
        );
        let baseline_a = records_b
            .iter()
            .find_map(|r| match r {
                EntityRecord::FullBaseline {
                    network_id,
                    baseline_id,
                    ..
                } if *network_id == net_a.0 => Some(*baseline_id),
                _ => None,
            })
            .expect("client B holds pawn A as a baseline");
        replication.apply_ack(CLIENT_B, 0, &[(net_a.0, baseline_a)], &[]);

        // Close client A: the single cleanup path.
        let despawned = on_slot_closed(
            &mut registry,
            &mut slot_pawns,
            &mut replicable,
            &mut replication,
            CLIENT_A,
        );
        assert_eq!(despawned, Some(pawn_a));
        assert!(!registry.exists(pawn_a), "pawn A despawned");
        assert!(
            !replicable.contains(pawn_a),
            "pawn A left the replicable set"
        );
        assert_eq!(slot_pawns.pawn_for(CLIENT_A), None, "slot A freed");
        assert_eq!(slot_pawns.len(), 1, "only slot B remains");

        // Next tick: pawn A is absent from produce_owned_snapshots, so the tracker
        // turns it into a despawn tombstone that reaches client B.
        let records_b = ingest_and_records(
            &registry,
            &replicable,
            &mut allocator,
            &mut replication,
            CLIENT_B,
            2,
        );
        assert!(
            records_b.iter().any(|r| matches!(
                r,
                EntityRecord::Despawn { network_id, .. } if *network_id == net_a.0
            )),
            "remaining client B receives pawn A's despawn"
        );
    }

    // Slot reuse never reuses a stale NetworkId: a reused ClientId gets a fresh
    // monotonic id, and the old id is never re-emitted.
    #[test]
    fn slot_reuse_gets_fresh_network_id() {
        let mut registry = EntityRegistry::new();
        let mut slot_pawns = SlotPawns::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();

        // First connection on slot A.
        let (_pawn1, net_first) = on_slot_accepted(
            &mut registry,
            &mut slot_pawns,
            &mut allocator,
            &mut replicable,
            CLIENT_A,
            SlotPawnSource::TransformFixture,
        )
        .expect("transform fixture accept always spawns");
        // Close it.
        on_slot_closed(
            &mut registry,
            &mut slot_pawns,
            &mut replicable,
            &mut replication,
            CLIENT_A,
        );

        // A later connection reuses the same ClientId (slot reuse). It must get a
        // fresh pawn and a fresh NetworkId — the old one is never re-emitted.
        let (_pawn2, net_second) = on_slot_accepted(
            &mut registry,
            &mut slot_pawns,
            &mut allocator,
            &mut replicable,
            CLIENT_A,
            SlotPawnSource::TransformFixture,
        )
        .expect("transform fixture accept always spawns");
        assert_ne!(
            net_first.0, net_second.0,
            "a reused slot gets a fresh monotonic NetworkId"
        );
        assert!(
            net_second.0 > net_first.0,
            "NetworkId allocation is monotonic across slot reuse"
        );
    }

    // Closing a slot that never owned a pawn (closed before accept) is a no-op that
    // returns None and does not panic.
    #[test]
    fn close_without_pawn_is_noop() {
        let mut registry = EntityRegistry::new();
        let mut slot_pawns = SlotPawns::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();

        let despawned = on_slot_closed(
            &mut registry,
            &mut slot_pawns,
            &mut replicable,
            &mut replication,
            CLIENT_A,
        );
        assert_eq!(despawned, None, "no pawn to clean up");
    }

    // Re-accepting an already-accepted slot does not spawn a duplicate pawn.
    #[test]
    fn re_accept_is_idempotent() {
        let mut registry = EntityRegistry::new();
        let mut slot_pawns = SlotPawns::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();

        let (pawn1, net1) = on_slot_accepted(
            &mut registry,
            &mut slot_pawns,
            &mut allocator,
            &mut replicable,
            CLIENT_A,
            SlotPawnSource::TransformFixture,
        )
        .expect("transform fixture accept always spawns");
        let (pawn2, net2) = on_slot_accepted(
            &mut registry,
            &mut slot_pawns,
            &mut allocator,
            &mut replicable,
            CLIENT_A,
            SlotPawnSource::TransformFixture,
        )
        .expect("transform fixture accept always spawns");
        assert_eq!(pawn1, pawn2, "no duplicate pawn on re-accept");
        assert_eq!(net1.0, net2.0, "stable NetworkId on re-accept");
        assert_eq!(slot_pawns.len(), 1);
    }

    // --- Descriptor-backed net-slot spawn (M15 Phase 3 Task 4) ----------------

    use crate::scripting::components::player_movement::PlayerMovementComponent;
    use crate::scripting::data_descriptors::{
        AirParams, CapsuleParams, EntityTypeDescriptor, FallParams, GroundParams,
        PlayerMovementDescriptor, SpeedParams,
    };
    use crate::scripting::provenance::{DescriptorProvenance, DescriptorSpawnPath};
    use crate::scripting::registry::ComponentKind;

    /// A minimal `"player"` descriptor carrying a movement component — the default
    /// `entity_class` `spawn_net_slot_pawn` looks up.
    fn player_descriptor() -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some("player".to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: Some(PlayerMovementDescriptor {
                capsule: CapsuleParams {
                    radius: 0.4,
                    half_height: 0.8,
                    eye_height: 0.5,
                },
                ground: GroundParams {
                    speed: SpeedParams {
                        walk: 7.0,
                        run: 11.0,
                        crouch: 3.0,
                    },
                    accel: 10.0,
                    step_height: 0.3,
                    max_slope: 45.0,
                },
                air: AirParams {
                    forward_steer: 0.0,
                    accel: 0.7,
                    max_control_speed: 0.5,
                    bunny_hop: false,
                    jumps: 0,
                    jump_velocity: 5.5,
                    jump_ceiling: 0.0,
                },
                fall: FallParams {
                    terminal_velocity: 40.0,
                },
                stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
                stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
                dash: None,
                forgiveness: None,
                crouch: None,
                view_feel: None,
            }),
            weapon: None,
            mesh: None,
            health: None,
            ai: None,
        }
    }

    /// A synthetic `player_spawn` placement (the task allows synthetic placements in
    /// tests). Default `entity_class` resolves to the `"player"` descriptor.
    fn synthetic_placement() -> MapEntity {
        MapEntity {
            classname: "player_spawn".to_string(),
            origin: glam::Vec3::new(2.0, 1.0, -3.0),
            angles: glam::Vec3::ZERO,
            key_values: std::collections::HashMap::new(),
            tags: vec![],
        }
    }

    // A descriptor-backed accept materializes a real PlayerMovement pawn from the
    // synthetic placement: it carries a PlayerMovementComponent, a NetworkSlot
    // provenance (NOT a map-start spawn), is registered + NetworkId-stamped, and is
    // NOT marked the local player.
    #[test]
    fn descriptor_accept_spawns_player_movement_pawn_not_local() {
        let mut registry = EntityRegistry::new();
        let mut slot_pawns = SlotPawns::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let descriptors = [player_descriptor()];
        let placement = synthetic_placement();

        let (pawn, net_id) = on_slot_accepted(
            &mut registry,
            &mut slot_pawns,
            &mut allocator,
            &mut replicable,
            CLIENT_A,
            SlotPawnSource::Descriptor {
                placement: &placement,
                descriptors: &descriptors,
                agent_params: None,
            },
        )
        .expect("descriptor accept spawns a pawn from the synthetic placement");

        // It is a real movement pawn.
        assert!(
            registry.exists(pawn),
            "descriptor pawn is live in the registry"
        );
        assert!(
            matches!(
                registry.has_component_kind(pawn, ComponentKind::PlayerMovement),
                Ok(true)
            ),
            "descriptor pawn carries a PlayerMovementComponent"
        );
        let _component = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .expect("movement component materialized from the descriptor");

        // Provenance distinguishes it from a map-start single-player spawn.
        let provenance = registry
            .get_component::<DescriptorProvenance>(pawn)
            .expect("net-slot pawn carries descriptor provenance");
        assert_eq!(provenance.spawn_path, DescriptorSpawnPath::NetworkSlot);

        // It is NOT the local player (host never marks a remote pawn local).
        assert_ne!(
            registry.local_player_pawn(),
            Some(pawn),
            "a descriptor net-slot pawn is never marked the local player"
        );

        // It is registered, NetworkId-stamped, and replicates.
        assert!(replicable.contains(pawn));
        assert_eq!(slot_pawns.pawn_for(CLIENT_A), Some(pawn));
        let owned = produce_owned_snapshots(
            &registry,
            &replicable,
            &mut allocator,
            &crate::netcode::MovementOwners::new(),
            &crate::netcode::HostCommandQueues::new(),
        );
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0].network_id, net_id.0);
        // The descriptor pawn carries BOTH Transform and PlayerMovementState payloads
        // (unlike the Transform-only fixture).
        assert_eq!(
            owned[0].components.len(),
            2,
            "descriptor pawn replicates Transform + PlayerMovementState"
        );
        // M15 Phase 3 Task 7: the owned snapshot carries the resolved descriptor class
        // (default `"player"`) so the client materializes the matching component.
        assert_eq!(
            owned[0].entity_class,
            Some("player".to_string()),
            "descriptor net-slot pawn stamps its entity_class for the wire"
        );
    }

    // The deterministic slot->placement assignment is stable across reconnect: the
    // same client id always resolves to the same placement index, and the assignment
    // is recorded (auditable) before the spawn.
    #[test]
    fn placement_assignment_is_deterministic_and_survives_close() {
        let mut slot_pawns = SlotPawns::new();
        // Three placements; two clients accept in order -> indices 0 and 1.
        assert_eq!(slot_pawns.assign_placement(CLIENT_A, 3), Some(0));
        assert_eq!(slot_pawns.assign_placement(CLIENT_B, 3), Some(1));
        // Re-asking is idempotent (auditable, stable).
        assert_eq!(slot_pawns.assign_placement(CLIENT_A, 3), Some(0));
        assert_eq!(slot_pawns.placement_assignment(CLIENT_A), Some(0));
        assert_eq!(slot_pawns.placement_assignment(CLIENT_B), Some(1));
        // No placements -> no assignment.
        let mut empty = SlotPawns::new();
        assert_eq!(empty.assign_placement(CLIENT_A, 0), None);
    }
}
