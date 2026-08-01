// Host tick, replication, and participation lifecycle glue.
// See: context/lib/networking.md

use super::*;

/// Microseconds per server sim tick (60 Hz), used to derive the telemetry-only
/// `server_echo_time_us` carried in a time-sync echo. Equal to the estimator's
/// [`timesync::DEFAULT_MICROS_PER_TICK`]; kept here so `main.rs` builds the
/// telemetry stamp without importing the net const directly.
pub(crate) const SERVER_TICK_MICROS: u64 = timesync::DEFAULT_MICROS_PER_TICK;

/// Snapshot send cadence: one snapshot per client every second 60 Hz sim tick
/// (30 Hz). The host ingests the registry every sim tick (so dirty detection sees
/// every change) but only encodes + sends on this cadence.
///
/// M15 Phase 3 calibration (playtest bug "Symptom 2", 2026-06-22): raised from every
/// third tick (20 Hz) to every second tick (30 Hz). The faster cadence shrinks the
/// snapshot-spacing contribution to remote-view latency (~50 ms half-period → ~33 ms,
/// so ~16 ms mean) and keeps two snapshots bracketing the now-tighter 50 ms
/// interpolation floor (`MIN_DELAY_MICROS`) so remote motion stays smooth. The +50%
/// snapshot bandwidth is acceptable for co-op's small player count.
pub(crate) const SNAPSHOT_TICK_INTERVAL: u32 = 2;

/// Advance the authoritative host tick and retain whether this redraw crossed a
/// snapshot-cadence edge. Catch-up frames call this once per completed fixed tick,
/// while the post-loop serializer consumes the accumulated `snapshot_due` bit once.
pub(crate) fn complete_host_fixed_tick(tick: &mut u32, snapshot_due: &mut bool) {
    *tick = tick.wrapping_add(1);
    *snapshot_due |= *tick % SNAPSHOT_TICK_INTERVAL == 0;
}

/// Host-only Phase 2 net-demo fixture state. Activation is a startup decision read
/// once from the environment; the spawned `EntityId` is filled in lazily on the first
/// host tick that has a registry to spawn into.
///
/// Gated to the demo/harness path only — `enabled` is false on an ordinary host, so a
/// production listen server never spawns the demo mover. This is deliberately an env
/// gate rather than a CLI flag or FGD entity: the mover is a throwaway demo fixture,
/// not an authored gameplay object, so it must not grow a permanent CLI/script/FGD
/// surface (entity_model.md §4 — no authored archetype).
pub(crate) struct DemoMoverState {
    enabled: bool,
    entity: Option<EntityId>,
}

impl DemoMoverState {
    /// Read the demo-mover activation from the environment. `POSTRETRO_NET_DEMO_MOVER=1`
    /// turns it on; anything else (unset, empty, other value) leaves it off.
    pub(crate) fn from_env() -> Self {
        let enabled = std::env::var("POSTRETRO_NET_DEMO_MOVER")
            .map(|v| v == "1")
            .unwrap_or(false);
        Self {
            enabled,
            entity: None,
        }
    }
}

/// Drive the host-only demo mover (Task 6, demo path only). On the first call with the
/// demo path active, spawns one deterministic AI-less mover, registers it in the
/// replicable set, and stamps its `NetworkId`; every call thereafter writes its
/// deterministic pose for `server_tick`. A no-op when the demo path is off.
///
/// Game-logic-owned: the spawn and the pose write flow through `EntityRegistry::spawn`
/// / `set_component`. The mover is a `Transform`-only entity (no movement payload), so
/// on the client it replicates as the dumb mover whose interpolation-buffer starvation
/// path holds the last pose.
pub(crate) fn host_drive_demo_mover(
    registry: &mut EntityRegistry,
    demo_mover: &mut DemoMoverState,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    server_tick: u32,
) {
    if !demo_mover.enabled {
        return;
    }
    let pose = DemoMover::pose_at(server_tick);
    match demo_mover.entity {
        Some(id) if registry.exists(id) => {
            // Steady state: write the deterministic pose for this tick.
            let _ = registry.set_component_value(id, ComponentValue::Transform(pose));
        }
        _ => {
            // First tick (or the entity vanished): spawn, register, stamp.
            let id = registry.spawn(pose);
            allocator.stamp(id);
            replicable.register(id);
            demo_mover.entity = Some(id);
            log::info!("[Net] demo mover spawned {id:?} (Phase 2 net-demo fixture)");
        }
    }
}

/// Drive one host sim tick of Phase 2 per-client delta replication. Game-logic
/// owned: borrows the registry immutably, copies the replicable set into owned
/// wire-mirror snapshots, releases the borrow, then feeds the net tracker and (on
/// the 30 Hz cadence) encodes + sends a per-client delta snapshot to every accepted
/// client.
///
/// `tick` is the monotonic fixed-simulation tick stamp. `snapshot_due` records that
/// at least one completed tick reached the cadence during this redraw. A snapshot
/// is still encoded at most once for `tick`, even if a caller reaches this path
/// more than once before another fixed tick completes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn host_replicate(
    registry: &EntityRegistry,
    slot_table: &SlotTable,
    server: &mut NetServer,
    allocator: &mut NetworkIdAllocator,
    replication: &mut ServerReplication,
    state_slots: &mut state_slots::HostStateReplication,
    replicable: &ReplicableSet,
    owners: &MovementOwners,
    weapon_owners: &WeaponOwners,
    command_queues: &HostCommandQueues,
    host_aim: Option<(EntityId, f32)>,
    tick: u32,
    snapshot_due: bool,
    last_emitted_snapshot_tick: &mut Option<u32>,
) -> Vec<EntityId> {
    // Owned post-tick snapshot rule: copy replicable state into owned mirrors keyed
    // by NetworkId while borrowing the registry, then release before the net call.
    // Owned movement pawns also carry their owner id + resolved cursor (Phase 3).
    let owned = replication::produce_owned_snapshots_with_host_aim(
        registry,
        replicable,
        allocator,
        owners,
        command_queues,
        host_aim,
    );
    replication.ingest_tick(owned);

    // Catch-up may finish past a cadence edge (for example 1 -> 2 -> 3). The
    // completed-tick seam retains that edge for this post-loop send.
    if !snapshot_due {
        return Vec::new();
    }

    let participating = server.participating_clients();
    if participating.is_empty() {
        return Vec::new();
    }
    if *last_emitted_snapshot_tick == Some(tick) {
        return Vec::new();
    }
    *last_emitted_snapshot_tick = Some(tick);

    // The replicated-state schema fingerprint is stamped into every snapshot carrying
    // state records so the client gates on a match. Built once from the live slot table.
    let state_fingerprint = state_slots.fingerprint(slot_table);
    // Ingest this frame's authoritative source values ONCE before the per-client loop:
    // the scan is frame-wide (every replicated slot, every owned pawn), so running it
    // per client would repeat it O(clients) times. Each client's `produce_for_client`
    // below only reads the now-ingested per-client view.
    let sampled_weapons = state_slots.ingest_frame_and_collect_sampled_weapons(
        slot_table,
        registry,
        owners,
        weapon_owners,
    );
    // One sequence shared across all clients in this 30 Hz batch — and shared with the
    // state tracker's `produce_for_client` so one ack describes one server frame.
    let sequence = replication.begin_batch();
    for client_id in participating {
        // Registration also occurs on participation entry. Keep this idempotent
        // send-side guard so a participating client always has baseline state.
        replication.register_client(client_id);
        state_slots.register_client(client_id);
        if let Some(mut raw) = replication.encode_in_batch(client_id, tick, sequence) {
            // Splice this client's replicated-state records into the SAME snapshot
            // envelope the entity tracker produced (no new channel, no sibling message).
            // The entity tracker leaves `state_records` empty + an all-zero fingerprint;
            // overwrite both with the real fingerprint and the per-client records.
            raw.state_schema_fingerprint = state_fingerprint;
            if let Some(records) = state_slots.produce_for_client(client_id, sequence) {
                raw.state_records = records;
            }
            let bytes = wire::encode(&raw);
            let _ = server.send_snapshot(client_id, bytes);
        }
    }
    sampled_weapons
}

/// Spawn and register the slot-owned pawn on entry to participation. The engine
/// drives this from ordered `SlotEvent::Participating` edges, including
/// re-promotion. It uses the same allocator, replicable set, and slot map as exit
/// cleanup. Idempotent per slot (see [`on_slot_accepted`]).
///
/// This glue path has no player descriptor, so the pawn is the `Transform`-only inert
/// fixture (entity_model.md §7b — not a real movement pawn). Called BEFORE the frame's
/// `host_replicate` so the new pawn is in the first snapshot.
///
/// Game-logic-owned: the spawn flows through `EntityRegistry::spawn`; the caller
/// threads in the mutable registry borrow so this module never reaches into `App`.
pub(crate) fn host_handle_accept(
    registry: &mut EntityRegistry,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    slot_pawns: &mut SlotPawns,
    client_id: u64,
) {
    let _ = on_slot_accepted(
        registry,
        slot_pawns,
        allocator,
        replicable,
        client_id,
        SlotPawnSource::TransformFixture,
    );
}

/// Production participation seam for a movement session: spawn the descriptor-backed
/// remote `PlayerMovement` pawn for a participating client. Deterministically assigns the
/// slot a `player_spawn` placement (auditable, stable across reconnect), records the
/// owner mapping, then materializes the pawn through [`on_slot_accepted`]'s descriptor
/// path. Falls back to nothing (logged) if there are no spawn points or the descriptor
/// spawn fails — the caller keeps the slot for a later retry.
///
/// `spawn_points` are the level's `player_spawn` placements; `descriptors` the
/// registered entity descriptors; `agent_params` the navmesh capsule (or `None`).
/// Game-logic-owned: the spawn flows through `EntityRegistry::spawn`; the caller
/// threads in the mutable registry borrow. Returns the materialized pawn so the
/// caller can resolve presentation bindings against the level-installed tables.
#[allow(clippy::too_many_arguments)]
pub(crate) fn host_handle_accept_descriptor(
    registry: &mut EntityRegistry,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    slot_pawns: &mut SlotPawns,
    command_queues: &mut HostCommandQueues,
    owners: &mut MovementOwners,
    weapon_owners: &mut WeaponOwners,
    open_shots: &mut OpenAuthorizedShots,
    pending_hit_declarations: &mut PendingHitDeclarations,
    weaponless_fire_logged: &mut std::collections::HashSet<EntityId>,
    client_id: u64,
    spawn_points: &[crate::scripting::map_entity::MapEntity],
    descriptors: &[EntityTypeDescriptor],
    agent_params: Option<NavAgentParams>,
    carried_loadout: Option<&super::CarriedState>,
) -> Option<EntityId> {
    host_handle_accept_descriptor_at_placement(
        registry,
        allocator,
        replicable,
        slot_pawns,
        command_queues,
        owners,
        weapon_owners,
        open_shots,
        pending_hit_declarations,
        weaponless_fire_logged,
        client_id,
        spawn_points,
        0,
        descriptors,
        agent_params,
        carried_loadout,
    )
}

/// Descriptor-backed remote spawn at a placement selected by the durable-seat
/// layer. The carried-loadout parameter remains entirely Task 5-owned; this
/// helper receives only the already-chosen map placement index.
#[allow(clippy::too_many_arguments)]
pub(crate) fn host_handle_accept_descriptor_at_placement(
    registry: &mut EntityRegistry,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    slot_pawns: &mut SlotPawns,
    command_queues: &mut HostCommandQueues,
    owners: &mut MovementOwners,
    weapon_owners: &mut WeaponOwners,
    open_shots: &mut OpenAuthorizedShots,
    pending_hit_declarations: &mut PendingHitDeclarations,
    weaponless_fire_logged: &mut std::collections::HashSet<EntityId>,
    client_id: u64,
    spawn_points: &[crate::scripting::map_entity::MapEntity],
    placement_index: usize,
    descriptors: &[EntityTypeDescriptor],
    agent_params: Option<NavAgentParams>,
    carried_loadout: Option<&super::CarriedState>,
) -> Option<EntityId> {
    cleanup_stale_slot_replacement(
        registry,
        allocator,
        replicable,
        slot_pawns,
        command_queues,
        owners,
        weapon_owners,
        open_shots,
        pending_hit_declarations,
        weaponless_fire_logged,
        client_id,
    );

    let Some(placement) = spawn_points.get(placement_index) else {
        log::warn!(
            "[Net] slot {client_id} accepted but player_spawn placement {placement_index} is unavailable; no pawn spawned"
        );
        return None;
    };

    let spawned = on_slot_accepted(
        registry,
        slot_pawns,
        allocator,
        replicable,
        client_id,
        SlotPawnSource::Descriptor {
            placement,
            descriptors,
            agent_params,
            carried_loadout,
        },
    );

    if let Some((pawn, _net_id)) = spawned {
        let entity_class = placement
            .key_values
            .get("entity_class")
            .map(String::as_str)
            .unwrap_or("player");
        remote_materialize::apply_remote_player_viewer_role(
            entity_class,
            descriptors,
            registry,
            pawn,
        );
        // Record the owner mapping (pawn -> client_id) so snapshot production can stamp
        // `owner_client_id` and the resolved cursor. The client's command queue is
        // created lazily on its first ingested command.
        owners.set(pawn, client_id);
        weapon_owners.mark_attachment_dirty(pawn);
        let _ = command_queues;
        return Some(pawn);
    }
    None
}

/// Register the listen host's OWN player pawn for OUTBOUND replication (M15 Phase 3,
/// issue 3b): without this, the host pawn never enters the `ReplicableSet`, so
/// `produce_owned_snapshots` never emits it and clients see no host capsule.
///
/// This is replication/presentation bookkeeping only. The host pawn keeps being driven LOCALLY by
/// `simulate_tick`/`local_movement_pawn` — it is deliberately NOT recorded in
/// `MovementOwners`, NOT command-queued, and NOT predicted/reconciled. Because it has
/// no `owner_client_id`, its per-recipient `local_player` flag is false for every
/// client (clients interpolate it as a normal remote pawn).
///
/// Idempotent and reload-safe: registering the same pawn twice is a no-op (the set and
/// the allocator are both stable per `EntityId`). On a level reload the freshly-spawned
/// pawn is a distinct `EntityId`, so the previously-tracked host pawn (if any) is
/// unregistered and marked for attachment removal first. The new pawn is marked for
/// inventory-derived attachment resolution here so ownership and presentation change
/// atomically.
///
/// Game-logic-owned: it reads the registry through the borrow the caller threads in and
/// only touches host bookkeeping; it never reaches into `App`.
pub(crate) fn host_register_own_pawn(
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    host_pawn: &mut Option<EntityId>,
    weapon_owners: &mut WeaponOwners,
    pawn: EntityId,
) {
    // A level reload spawns a fresh host pawn (distinct EntityId). Drop the stale
    // registration before registering the new one so the replicable set never names a
    // despawned id. Re-registering the SAME pawn skips the churn (idempotent install).
    if let Some(previous) = *host_pawn {
        if previous != pawn {
            replicable.unregister(previous);
            allocator.forget(previous);
            weapon_owners.remove_pawn(previous);
        }
    }
    // Stamp the stable session-monotonic NetworkId and register for replication,
    // mirroring `on_slot_accepted` — but with NO owner mapping, so the host pawn is
    // replicated as an unowned (never-local) remote pawn to every client.
    let net_id = allocator.stamp(pawn);
    replicable.register(pawn);
    *host_pawn = Some(pawn);
    weapon_owners.mark_attachment_dirty(pawn);
    log::info!("[Net] host registered own pawn {pawn:?} as {net_id:?} (outbound replication only)");
}

/// Remove the listen host's prior local pawn from replication and weapon ownership.
/// Level install calls this when the replacement map has no player spawn, so stale
/// ownership cannot survive merely because there is no new pawn to register.
pub(crate) fn host_unregister_own_pawn(
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    host_pawn: &mut Option<EntityId>,
    weapon_owners: &mut WeaponOwners,
) -> Option<EntityId> {
    let previous = host_pawn.take()?;
    replicable.unregister(previous);
    allocator.forget(previous);
    weapon_owners.remove_pawn(previous);
    Some(previous)
}

/// Apply participation exits to the host's remote-pawn state. Close and demotion
/// share this cleanup: despawn the slot pawn, drop it from replication, and clear
/// ownership, command, state-slot, tuning, and combat bookkeeping.
///
/// Game-logic-owned: the registry mutation flows through `EntityRegistry::despawn`.
/// The mutable registry borrow is threaded in by the caller so this module never
/// reaches into `App`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn host_handle_lifecycle(
    registry: &mut EntityRegistry,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    replication: &mut ServerReplication,
    state_slots: &mut state_slots::HostStateReplication,
    slot_pawns: &mut SlotPawns,
    command_queues: &mut HostCommandQueues,
    owners: &mut MovementOwners,
    weapon_owners: &mut WeaponOwners,
    open_shots: &mut OpenAuthorizedShots,
    pending_hit_declarations: &mut PendingHitDeclarations,
    weaponless_fire_logged: &mut std::collections::HashSet<EntityId>,
    last_sent_tuning: &mut HashMap<u64, TuningPayload>,
    mut seat_table: Option<&mut SeatTable>,
    lifecycle: &[postretro_net::slots::SlotEvent],
) {
    use postretro_net::slots::SlotEvent;
    for event in lifecycle {
        match event {
            SlotEvent::Closed { client_id, .. } | SlotEvent::Demoted { client_id, .. } => {
                let previous_pawn = slot_pawns.pawn_for(*client_id);
                if let (Some(seats), Some(pawn)) = (seat_table.as_deref_mut(), previous_pawn) {
                    seats.harvest_pawn(registry, pawn);
                }
                if let Some(pawn) = previous_pawn {
                    pending_hit_declarations.remove_pawn_shots(allocator, pawn);
                }
                let despawned = on_slot_closed(
                    registry,
                    slot_pawns,
                    allocator,
                    replicable,
                    replication,
                    *client_id,
                );
                // M15 Phase 3: drop the closed client's command queue and the pawn's
                // owner mapping so its stale authority metadata never rides a later
                // snapshot. The slot's placement assignment is intentionally retained
                // (a reconnecting client lands on its prior spawn — auditable source).
                command_queues.remove_client(*client_id);
                // M15 Phase 3.5: drop the closed client's replicated-state baselines and
                // its owner-private slot values so none leak past the connection.
                state_slots.remove_client(*client_id);
                last_sent_tuning.remove(client_id);
                if let Some(pawn) = despawned {
                    cleanup_remote_pawn_owned_state(
                        registry,
                        allocator,
                        replicable,
                        owners,
                        weapon_owners,
                        open_shots,
                        pending_hit_declarations,
                        weaponless_fire_logged,
                        *client_id,
                        pawn,
                        &[],
                    );
                } else if let Some(pawn) = previous_pawn {
                    cleanup_remote_pawn_owned_state(
                        registry,
                        allocator,
                        replicable,
                        owners,
                        weapon_owners,
                        open_shots,
                        pending_hit_declarations,
                        weaponless_fire_logged,
                        *client_id,
                        pawn,
                        &[],
                    );
                }
            }
            // Entry is handled by the App's participation seam, which registers
            // replication and spawns the pawn. Kept exhaustive so a new slot event
            // is a compile error here as well.
            SlotEvent::Participating { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_handle_accept_spawns_registered_replicable_pawn_with_network_id() {
        let mut registry = EntityRegistry::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut slot_pawns = SlotPawns::new();
        const CLIENT_ID: u64 = 42;

        // Drive the accept through the production dispatch helper (NOT on_slot_accepted).
        host_handle_accept(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut slot_pawns,
            CLIENT_ID,
        );

        // A slot-owned pawn now exists for the client and is live in the registry.
        let pawn = slot_pawns
            .pawn_for(CLIENT_ID)
            .expect("accept spawned a slot-owned pawn for the client");
        assert!(
            registry.exists(pawn),
            "the slot pawn is live in the registry"
        );

        // It is registered for replication.
        assert!(
            replicable.contains(pawn),
            "the accepted pawn is in the replicable set"
        );

        // It has an allocated NetworkId and replicates: produce_owned_snapshots emits
        // exactly the one pawn, keyed by its allocated NetworkId.
        let expected_net_id = allocator.stamp(pawn);
        assert_eq!(
            allocator.network_id_for_entity(pawn),
            Some(expected_net_id),
            "host can name the pawn on the wire"
        );
        assert_eq!(
            allocator.entity_for_network_id(expected_net_id),
            Some(pawn),
            "host can resolve a declared target NetworkId"
        );
        let owned = produce_owned_snapshots(
            &registry,
            &replicable,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        assert_eq!(owned.len(), 1, "exactly the accepted pawn replicates");
        assert_eq!(
            owned[0].network_id, expected_net_id.0,
            "the replicated pawn carries its allocated NetworkId"
        );

        allocator.forget(pawn);
        assert_eq!(allocator.network_id_for_entity(pawn), None);
        assert_eq!(allocator.entity_for_network_id(expected_net_id), None);
    }

    #[test]
    fn ordered_participation_then_demotion_leaves_no_host_pawn() {
        use postretro_net::handshake::HoldingCause;
        use postretro_net::slots::SlotEvent;

        const CLIENT_ID: u64 = 43;
        let events = [
            SlotEvent::Participating {
                client_id: CLIENT_ID,
            },
            SlotEvent::Demoted {
                client_id: CLIENT_ID,
                cause: HoldingCause::HostLevelAbsent,
            },
        ];
        let mut registry = EntityRegistry::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();
        let mut state_slots = state_slots::HostStateReplication::new();
        let mut slot_pawns = SlotPawns::new();
        let mut command_queues = HostCommandQueues::new();
        let mut owners = MovementOwners::new();
        let mut weapon_owners = WeaponOwners::new();
        let mut open_shots = OpenAuthorizedShots::new();
        let mut pending_hit_declarations = PendingHitDeclarations::new();
        let mut weaponless_fire_logged = std::collections::HashSet::new();
        let mut last_sent_tuning = HashMap::new();

        for event in &events {
            host_handle_lifecycle(
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
                std::slice::from_ref(event),
            );
            if let SlotEvent::Participating { client_id } = event {
                replication.register_client(*client_id);
                state_slots.register_client(*client_id);
                host_handle_accept(
                    &mut registry,
                    &mut allocator,
                    &mut replicable,
                    &mut slot_pawns,
                    *client_id,
                );
            }
        }

        assert!(
            slot_pawns.pawn_for(CLIENT_ID).is_none(),
            "final admitted state owns no pawn"
        );
        assert!(
            replicable.iter().next().is_none(),
            "demotion removes the just-spawned pawn from replication"
        );
    }

    #[test]
    fn lifecycle_demotion_harvests_bound_pawn_health_before_despawn() {
        use postretro_entities::components::health::HealthComponent;
        use postretro_foundation::Seat;
        use postretro_scripting_core::data_descriptors::HealthDescriptor;

        const CLIENT_ID: u64 = 44;
        let mut registry = EntityRegistry::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();
        let mut state_slots = state_slots::HostStateReplication::new();
        let mut slot_pawns = SlotPawns::new();
        let mut command_queues = HostCommandQueues::new();
        let mut owners = MovementOwners::new();
        let mut weapon_owners = WeaponOwners::new();
        let mut open_shots = OpenAuthorizedShots::new();
        let mut pending_hit_declarations = PendingHitDeclarations::new();
        let mut weaponless_fire_logged = std::collections::HashSet::new();
        let mut last_sent_tuning = HashMap::new();
        let mut seats = SeatTable::from_test_session_id([5; 16]);
        let seat = seats
            .mint_admitted(CLIENT_ID, None, false)
            .expect("seat namespace has room");

        host_handle_accept(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut slot_pawns,
            CLIENT_ID,
        );
        let pawn = slot_pawns
            .pawn_for(CLIENT_ID)
            .expect("accepted pawn exists");
        let mut health = HealthComponent::from_descriptor(&HealthDescriptor {
            max: 100.0,
            hitbox: None,
            zone_multipliers: HashMap::new(),
        });
        health.current = 31.0;
        registry.set_component(pawn, health).unwrap();
        seats.bind_pawn(seat, pawn);

        host_handle_lifecycle(
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
            Some(&mut seats),
            &[postretro_net::slots::SlotEvent::Demoted {
                client_id: CLIENT_ID,
                cause: postretro_net::wire::HoldingCause::HostLevelAbsent,
            }],
        );

        assert!(!registry.exists(pawn), "demotion despawns the remote pawn");
        let carried_health = seats
            .carried_state(seat)
            .and_then(|state| state.health_current)
            .expect("harvest precedes the pawn despawn");
        assert!(
            (carried_health - 31.0).abs() <= 1.0e-6,
            "expected carried health 31.0, got {carried_health}"
        );
        assert_ne!(
            seat,
            Seat(0),
            "remote admission never aliases the local seat"
        );
    }
}
