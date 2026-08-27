// Host connection-slot lifecycle owns remote pawn and sibling weapon materialization and cleanup.
// See: context/lib/networking.md §Game-logic-owned apply invariant · context/lib/entity_model.md §3

use std::collections::HashMap;

use postretro_entities::components::inventory::Inventory;
use postretro_entities::{EntityId, EntityRegistry, EntityTypeDescriptor, Transform};
use postretro_foundation::NavAgentParams;
use postretro_net::replication::ServerReplication;
use postretro_net::wire::NetworkId;

use crate::scripting::builtins::net_descriptor::spawn_net_slot_pawn_with_carried_loadout;
use crate::scripting::map_entity::MapEntity;

use super::{NetworkIdAllocator, ReplicableSet};

/// The host-side slot -> remote-pawn map. One slot-owned pawn per accepted
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
    /// Last materialized sibling ids for each slot pawn. Inventory remains the
    /// authority while the pawn is live; this is only a teardown fallback when
    /// another system has already despawned the pawn before its slot closes.
    /// Keeping the ids with the slot, rather than in `WeaponOwners`, preserves
    /// the latter as a presentation-only dirty attachment queue.
    wieldables: HashMap<u64, Vec<EntityId>>,
}

impl SlotPawns {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The pawn entity for a slot, if one is registered. Used by lifecycle tests and
    /// available to the host owner-lookup path.
    #[allow(dead_code)]
    pub(crate) fn pawn_for(&self, client_id: u64) -> Option<EntityId> {
        self.pawns.get(&client_id).copied()
    }

    pub(crate) fn remove_client(&mut self, client_id: u64) -> Option<(EntityId, Vec<EntityId>)> {
        self.pawns.remove(&client_id).map(|pawn| {
            let wieldables = self.wieldables.remove(&client_id).unwrap_or_default();
            (pawn, wieldables)
        })
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
        carried_loadout: Option<&'a super::CarriedState>,
    },
}

/// React to a slot being accepted: create the slot-owned pawn, stamp it with a
/// fresh session-monotonic `NetworkId`, add it to the replicable set, and record the
/// slot mapping. Returns the pawn `EntityId` and its assigned `NetworkId`.
///
/// Idempotent per slot: a second accept for an already-mapped, still-live slot
/// returns the existing pawn without spawning a duplicate. A re-accept whose mapped
/// pawn has gone stale (despawned out from under us) re-spawns a fresh one.
///
/// The host is authoritative for the pawn's simulation: it applies the owning
/// client's movement and firing-slot input, and processes reload and switch
/// declarations. The `NetworkId` is allocated by the shared monotonic allocator,
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
            carried_loadout,
        } => {
            let Some(id) = spawn_net_slot_pawn_with_carried_loadout(
                placement,
                descriptors,
                registry,
                agent_params,
                carried_loadout,
            ) else {
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
    let wieldables = registry
        .get_component::<Inventory>(pawn)
        .ok()
        .map(|inventory| inventory.wieldables.iter().flatten().copied().collect())
        .unwrap_or_default();
    slot_pawns.pawns.insert(client_id, pawn);
    slot_pawns.wieldables.insert(client_id, wieldables);
    log::info!("[Net] slot {client_id} accepted: spawned remote pawn {pawn:?} as {net_id:?}");
    Some((pawn, net_id))
}

/// The result of closing a slot: the despawned pawn plus the `NetworkId` its
/// remote-presentation buffer was keyed by.
///
/// The caller uses `network_id` to forget the pawn's entry in the host's
/// `client_pawn_presentation` interpolation buffer, mirroring the client's
/// per-despawn buffer forget. `network_id` is `None` only when the closed pawn had
/// never been stamped (no id was allocated, so nothing is buffered under it).
pub(crate) struct ClosedSlotPawn {
    /// The despawned pawn entity.
    pub(crate) pawn: EntityId,
    /// The pawn's stable `NetworkId`, captured BEFORE `allocator.forget` removed the
    /// mapping. `None` when the pawn was never stamped.
    pub(crate) network_id: Option<NetworkId>,
}

/// Close a slot and clean up its pawn, replication state, and slot mapping.
///
/// Returns the despawned pawn's `EntityId` and its `NetworkId`, or `None` when the
/// slot never owned a pawn. The pawn leaves the next snapshot, which emits a despawn
/// tombstone.
///
/// Suspend demotes peers before the next transport poll. The durable seat map
/// outlives that reset and can still name the old pawn, so disconnect cleanup
/// supplies it here as a fallback rather than leaving an orphan in the registry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn on_slot_closed_with_fallback(
    registry: &mut EntityRegistry,
    slot_pawns: &mut SlotPawns,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    replication: &mut ServerReplication,
    client_id: u64,
    fallback_pawn: Option<EntityId>,
) -> Option<ClosedSlotPawn> {
    // Drop the closed client's per-client replication state regardless of whether it
    // owned a pawn: it will never ack again.
    replication.remove_client(client_id);

    let mapped_pawn = slot_pawns.pawns.remove(&client_id);
    let pawn = mapped_pawn.or(fallback_pawn)?;
    let retained_wieldables = mapped_pawn
        .and_then(|_| slot_pawns.wieldables.remove(&client_id))
        .unwrap_or_default();
    // Inventory owns every sibling wieldable. Snapshot their ids before the pawn
    // dies, then despawn every instance before clearing the owner; after pawn
    // despawn its component column is inaccessible by design.
    let wieldables: Vec<EntityId> = registry
        .get_component::<Inventory>(pawn)
        .ok()
        .map(|inventory| inventory.wieldables.iter().flatten().copied().collect())
        .unwrap_or(retained_wieldables);
    // Resolve the pawn's NetworkId BEFORE `allocator.forget(pawn)` below removes the
    // mapping. The caller forgets this id from the host's remote-presentation buffer.
    let network_id = allocator.network_id_for_entity(pawn);
    // Remove from the replicable set FIRST so the next ingest sees the entity gone
    // and emits the despawn tombstone; then despawn through game logic. (Order does
    // not matter for correctness here since both run before the next ingest, but
    // unregistering first keeps the invariant "replicable set never names a
    // despawned id" true at every yield point.)
    replicable.unregister(pawn);
    allocator.forget(pawn);
    for wieldable in wieldables {
        replicable.unregister(wieldable);
        allocator.forget(wieldable);
        let _ = registry.despawn(wieldable);
    }
    // `despawn` errors only on a stale id; the pawn may already be gone if game logic
    // despawned it. Either way the post-state is "gone", so the error is swallowed.
    let _ = registry.despawn(pawn);
    log::info!("[Net] slot {client_id} closed: despawned remote pawn {pawn:?}");
    Some(ClosedSlotPawn { pawn, network_id })
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
        // The lifecycle glue is cause-agnostic: cleanup takes no cause. The
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
        let despawned = on_slot_closed_with_fallback(
            &mut registry,
            &mut slot_pawns,
            &mut allocator,
            &mut replicable,
            &mut replication,
            CLIENT_A,
            None,
        );
        assert_eq!(despawned.map(|closed| closed.pawn), Some(pawn_a));
        assert!(!registry.exists(pawn_a), "pawn A despawned");
        assert!(
            !replicable.contains(pawn_a),
            "pawn A left the replicable set"
        );
        assert_eq!(slot_pawns.pawn_for(CLIENT_A), None, "slot A freed");
        assert!(
            !allocator.maps_entity(pawn_a) && !allocator.maps_network_id(net_a),
            "allocator forward and reverse maps forget the despawned pawn"
        );
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
        on_slot_closed_with_fallback(
            &mut registry,
            &mut slot_pawns,
            &mut allocator,
            &mut replicable,
            &mut replication,
            CLIENT_A,
            None,
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
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();

        let despawned = on_slot_closed_with_fallback(
            &mut registry,
            &mut slot_pawns,
            &mut allocator,
            &mut replicable,
            &mut replication,
            CLIENT_A,
            None,
        );
        assert!(despawned.is_none(), "no pawn to clean up");
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

    use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
    use postretro_entities::{ComponentKind, EntityTypeDescriptor};
    use postretro_foundation::{
        AirParams, CapsuleParams, FallParams, FireMode, GroundParams, PlayerMovementComponent,
        PlayerMovementDescriptor, ResolutionMode, SpeedParams, WeaponDescriptor,
    };

    /// A minimal `"player"` descriptor carrying a movement component — the default
    /// `entity_class` `spawn_net_slot_pawn` looks up.
    fn player_descriptor() -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some("player".to_string()),
            inventory: None,
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
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn player_with_default_weapon(default_weapon: &str) -> EntityTypeDescriptor {
        player_with_loadout(&[default_weapon])
    }

    fn player_with_loadout(loadout: &[&str]) -> EntityTypeDescriptor {
        let mut descriptor = player_descriptor();
        descriptor.inventory = Some(postretro_entities::InventoryDescriptor {
            loadout: loadout.iter().map(|weapon| (*weapon).to_string()).collect(),
        });
        descriptor
    }

    fn weapon_descriptor(classname: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: Some(WeaponDescriptor {
                damage: 10.0,
                pellet_count: 1,
                spread_degrees: 0.0,
                range: 64.0,
                cooldown_ms: 120.0,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Hitscan,
                projectile: None,
                credit_source: None,
                third_person_model: None,
                viewmodel: None,
                placement: None,
                muzzle_offset: None,
                resource: None,
                lower_ms: 0,
                raise_ms: 0,
                block_during_reload: None,
            }),
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
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
                carried_loadout: None,
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
            Some("descriptor:player".to_string()),
            "descriptor net-slot pawn stamps its entity_class for the wire"
        );
    }

    // Demotion is the same exit from participation as a close, so gameplay must
    // clear every slot-owned domain while the transport remains alive.
    #[test]
    fn o22_o24_demotion_mid_switch_clears_then_repromotion_builds_fresh_inventory() {
        let mut registry = EntityRegistry::new();
        let mut slot_pawns = SlotPawns::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();
        let mut state_slots = crate::netcode::state_slots::HostStateReplication::new();
        let mut command_queues = crate::netcode::HostCommandQueues::new();
        let mut owners = crate::netcode::MovementOwners::new();
        let mut weapon_owners = crate::netcode::WeaponOwners::new();
        let mut open_shots = crate::netcode::OpenAuthorizedShots::new();
        let mut pending_hit_declarations = crate::netcode::PendingHitDeclarations::new();
        let mut weaponless_fire_logged = std::collections::HashSet::new();
        let descriptors = [
            player_with_loadout(&["reference_pistol", "reference_pistol"]),
            weapon_descriptor("reference_pistol"),
        ];
        let spawn_points = [synthetic_placement()];

        crate::netcode::host_handle_accept_descriptor_at_placement(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut slot_pawns,
            &mut command_queues,
            &mut owners,
            &mut weapon_owners,
            &mut open_shots,
            &mut pending_hit_declarations,
            &mut weaponless_fire_logged,
            CLIENT_A,
            &spawn_points,
            0,
            &descriptors,
            None,
            None,
        );

        let pawn = slot_pawns
            .pawn_for(CLIENT_A)
            .expect("descriptor accept spawned a slot pawn");
        let weapons: Vec<EntityId> = registry
            .get_component::<Inventory>(pawn)
            .expect("descriptor accept materializes pawn inventory")
            .wieldables
            .iter()
            .flatten()
            .copied()
            .collect();
        assert_eq!(weapons.len(), 2, "fixture equips two inventory slots");
        let mut lowering = registry
            .get_component::<postretro_entities::components::weapon::WeaponComponent>(weapons[0])
            .unwrap()
            .clone();
        lowering.state = postretro_entities::components::wieldable_state::WieldableState::Lowering;
        lowering.state_total_ms = 40;
        lowering.state_remaining_ms = 20;
        registry.set_component(weapons[0], lowering).unwrap();
        let mut switching_inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
        switching_inventory.switch_target = Some(1);
        switching_inventory.switch_origin = Some(0);
        registry.set_component(pawn, switching_inventory).unwrap();
        let weapon = weapons[0];
        let pawn_net = allocator
            .network_id_for_entity(pawn)
            .expect("accepted pawn is stamped");
        let weapon_nets: Vec<NetworkId> = weapons
            .iter()
            .map(|&weapon| {
                let net_id = allocator.stamp(weapon);
                replicable.register(weapon);
                net_id
            })
            .collect();
        let shot_id = crate::netcode::ShotId::from_parts(pawn_net, 5);
        open_shots.record(
            crate::netcode::AuthorizedShot {
                shot_id,
                pawn,
                weapon,
                fire_tick: 1,
                damage: 10.0,
                range: 64.0,
                pellet_count: 1,
                credit_source: "weapon.test.lifecycle".to_string(),
                is_projectile: false,
                fire_origin: glam::Vec3::ZERO,
                timeout_budget_ticks: crate::netcode::MAX_OPEN_SHOT_AGE_TICKS,
            },
            CLIENT_A,
        );
        pending_hit_declarations.push(
            CLIENT_A,
            postretro_net::wire::HitDeclaration {
                shot_id: shot_id.raw(),
                records: Vec::new(),
            },
        );
        weaponless_fire_logged.insert(pawn);
        assert_eq!(
            owners.owner_of(pawn),
            Some(CLIENT_A),
            "movement owner still records the accepting client"
        );
        assert!(matches!(
            registry.has_component_kind(weapon, ComponentKind::Weapon),
            Ok(true)
        ));

        let snapshots = crate::netcode::replication::produce_owned_snapshots_with_host_aim(
            &registry,
            &replicable,
            &mut allocator,
            &owners,
            &command_queues,
            None,
        );
        let pawn_snapshot = snapshots
            .iter()
            .find(|snapshot| snapshot.network_id == pawn_net.0)
            .expect("the active-weapon pawn is replicated");
        assert_eq!(
            pawn_snapshot.active_weapon_archetype,
            Some("reference_pistol".to_string()),
            "the pawn inventory resolves the weapon descriptor's canonical name onto the snapshot"
        );

        let host_pitch_snapshots =
            crate::netcode::replication::produce_owned_snapshots_with_host_aim(
                &registry,
                &replicable,
                &mut allocator,
                &crate::netcode::MovementOwners::new(),
                &command_queues,
                Some((pawn, -0.37)),
            );
        let host_movement = host_pitch_snapshots
            .iter()
            .find(|snapshot| snapshot.network_id == pawn_net.0)
            .and_then(|snapshot| {
                snapshot
                    .components
                    .iter()
                    .find_map(|payload| match payload {
                        postretro_net::wire::ComponentPayload::PlayerMovementState(movement) => {
                            Some(movement)
                        }
                        _ => None,
                    })
            })
            .expect("listen-host pawn carries movement presentation state");
        assert!((host_movement.aim_pitch + 0.37).abs() <= 1.0e-6);

        let mut last_sent_tuning = std::collections::HashMap::new();
        crate::netcode::host_handle_lifecycle(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &mut state_slots,
            &mut slot_pawns,
            &mut command_queues,
            &mut owners,
            &mut weapon_owners,
            &mut open_shots,
            &mut pending_hit_declarations,
            &mut weaponless_fire_logged,
            &mut last_sent_tuning,
            None,
            &[postretro_net::slots::SlotEvent::Demoted {
                client_id: CLIENT_A,
                cause: postretro_net::wire::HoldingCause::HostLevelAbsent,
            }],
        );

        assert_eq!(
            owners.owner_of(pawn),
            None,
            "slot close clears movement ownership"
        );
        assert!(
            weapons.iter().all(|&weapon| !registry.exists(weapon)),
            "slot close despawns every descriptor-spawned inventory sibling"
        );
        assert!(
            !allocator.maps_entity(pawn)
                && !allocator.maps_network_id(pawn_net)
                && weapons
                    .iter()
                    .zip(&weapon_nets)
                    .all(|(&weapon, net_id)| !allocator.maps_entity(weapon)
                        && !allocator.maps_network_id(*net_id)),
            "slot close clears allocator forward and reverse mappings"
        );
        assert!(!replicable.contains(pawn));
        assert!(weapons.iter().all(|&weapon| !replicable.contains(weapon)));
        assert_eq!(open_shots.len(), 0);
        assert_eq!(pending_hit_declarations.len(), 0);
        assert!(
            !weaponless_fire_logged.contains(&pawn),
            "slot close clears weaponless-fire latch"
        );

        crate::netcode::host_handle_accept_descriptor_at_placement(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut slot_pawns,
            &mut command_queues,
            &mut owners,
            &mut weapon_owners,
            &mut open_shots,
            &mut pending_hit_declarations,
            &mut weaponless_fire_logged,
            CLIENT_A,
            &spawn_points,
            0,
            &descriptors,
            None,
            None,
        );
        let replacement = slot_pawns
            .pawn_for(CLIENT_A)
            .expect("re-promotion materializes a fresh pawn");
        assert_ne!(replacement, pawn);
        let replacement_inventory = registry
            .get_component::<Inventory>(replacement)
            .expect("fresh tuning/loadout materializes a new inventory");
        assert_eq!(replacement_inventory.active_slot, 0);
        assert_eq!(replacement_inventory.switch_target, None);
        assert_eq!(replacement_inventory.switch_origin, None);
        assert!(
            replacement_inventory
                .wieldables
                .iter()
                .flatten()
                .all(|weapon| !weapons.contains(weapon)),
            "no pre-demotion instance or slot holder survives re-promotion"
        );
    }

    #[test]
    fn o23_slot_close_after_pawn_despawn_removes_all_three_inventory_instances() {
        let mut registry = EntityRegistry::new();
        let mut slot_pawns = SlotPawns::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();
        let descriptors = [
            player_with_loadout(&["reference_pistol", "reference_pistol", "reference_pistol"]),
            weapon_descriptor("reference_pistol"),
        ];

        let (pawn, _) = on_slot_accepted(
            &mut registry,
            &mut slot_pawns,
            &mut allocator,
            &mut replicable,
            CLIENT_A,
            SlotPawnSource::Descriptor {
                placement: &synthetic_placement(),
                descriptors: &descriptors,
                agent_params: None,
                carried_loadout: None,
            },
        )
        .expect("descriptor slot materializes");
        let weapons = registry
            .get_component::<Inventory>(pawn)
            .unwrap()
            .wieldables
            .iter()
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(weapons.len(), 3);
        registry
            .despawn(pawn)
            .expect("fixture reproduces pawn-before-slot cleanup ordering");

        assert_eq!(
            on_slot_closed_with_fallback(
                &mut registry,
                &mut slot_pawns,
                &mut allocator,
                &mut replicable,
                &mut replication,
                CLIENT_A,
                None,
            )
            .map(|closed| closed.pawn),
            Some(pawn)
        );
        assert!(
            weapons.iter().all(|weapon| !registry.exists(*weapon)),
            "the slot's retained teardown ids remove every sibling after the pawn is gone"
        );
    }

    #[test]
    fn descriptor_stale_replacement_cleans_old_weapon_and_combat_state() {
        let mut registry = EntityRegistry::new();
        let mut slot_pawns = SlotPawns::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut command_queues = crate::netcode::HostCommandQueues::new();
        let mut owners = crate::netcode::MovementOwners::new();
        let mut weapon_owners = crate::netcode::WeaponOwners::new();
        let mut open_shots = crate::netcode::OpenAuthorizedShots::new();
        let mut pending_hit_declarations = crate::netcode::PendingHitDeclarations::new();
        let mut weaponless_fire_logged = std::collections::HashSet::new();
        let descriptors = [
            player_with_default_weapon("reference_pistol"),
            weapon_descriptor("reference_pistol"),
        ];
        let spawn_points = [synthetic_placement()];

        crate::netcode::host_handle_accept_descriptor_at_placement(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut slot_pawns,
            &mut command_queues,
            &mut owners,
            &mut weapon_owners,
            &mut open_shots,
            &mut pending_hit_declarations,
            &mut weaponless_fire_logged,
            CLIENT_A,
            &spawn_points,
            0,
            &descriptors,
            None,
            None,
        );
        let old_pawn = slot_pawns.pawn_for(CLIENT_A).expect("first pawn");
        let old_weapon = crate::netcode::active_wieldable_for_pawn(&registry, old_pawn)
            .expect("first inventory weapon");
        let mut reloading_weapon = registry
            .get_component::<postretro_entities::components::weapon::WeaponComponent>(old_weapon)
            .unwrap()
            .clone();
        reloading_weapon.state_remaining_ms = 500;
        reloading_weapon.state_total_ms = 1000;
        reloading_weapon.state =
            postretro_entities::components::wieldable_state::WieldableState::Reloading;
        registry
            .set_component(old_weapon, reloading_weapon)
            .unwrap();
        let old_pawn_net = allocator.network_id_for_entity(old_pawn).unwrap();
        let old_weapon_net = allocator.stamp(old_weapon);
        replicable.register(old_weapon);
        let shot_id = crate::netcode::ShotId::from_parts(old_pawn_net, 9);
        open_shots.record(
            crate::netcode::AuthorizedShot {
                shot_id,
                pawn: old_pawn,
                weapon: old_weapon,
                fire_tick: 2,
                damage: 10.0,
                range: 64.0,
                pellet_count: 1,
                credit_source: "weapon.test.lifecycle".to_string(),
                is_projectile: false,
                fire_origin: glam::Vec3::ZERO,
                timeout_budget_ticks: crate::netcode::MAX_OPEN_SHOT_AGE_TICKS,
            },
            CLIENT_A,
        );
        pending_hit_declarations.push(
            CLIENT_A,
            postretro_net::wire::HitDeclaration {
                shot_id: shot_id.raw(),
                records: Vec::new(),
            },
        );
        weaponless_fire_logged.insert(old_pawn);

        registry
            .despawn(old_pawn)
            .expect("test simulates stale externally-despawned slot pawn");
        crate::netcode::host_handle_accept_descriptor_at_placement(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut slot_pawns,
            &mut command_queues,
            &mut owners,
            &mut weapon_owners,
            &mut open_shots,
            &mut pending_hit_declarations,
            &mut weaponless_fire_logged,
            CLIENT_A,
            &spawn_points,
            0,
            &descriptors,
            None,
            None,
        );

        let replacement = slot_pawns
            .pawn_for(CLIENT_A)
            .expect("replacement pawn spawned");
        assert_ne!(replacement, old_pawn);
        assert!(registry.exists(replacement));
        assert!(
            !registry.exists(old_weapon),
            "stale pawn cleanup must not strand a reload-locked sibling weapon"
        );
        assert_eq!(owners.owner_of(old_pawn), None);
        assert!(!allocator.maps_entity(old_pawn));
        assert!(!allocator.maps_network_id(old_pawn_net));
        assert!(!allocator.maps_entity(old_weapon));
        assert!(!allocator.maps_network_id(old_weapon_net));
        assert!(!replicable.contains(old_pawn));
        assert!(!replicable.contains(old_weapon));
        assert_eq!(open_shots.len(), 0);
        assert_eq!(pending_hit_declarations.len(), 0);
        assert!(!weaponless_fire_logged.contains(&old_pawn));
    }
}
