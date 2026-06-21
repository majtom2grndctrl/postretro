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
}

impl SlotPawns {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The pawn entity for a slot, if one is registered.
    #[cfg(test)]
    pub(crate) fn pawn_for(&self, client_id: u64) -> Option<EntityId> {
        self.pawns.get(&client_id).copied()
    }

    /// Number of live slot pawns. Test-only assertion helper.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.pawns.len()
    }
}

/// Where a slot-owned pawn comes from. Phase 2 uses the `Transform`-only fixture in
/// the absence of a descriptor-backed pawn in this glue path; the descriptor-backed
/// variant is the preferred path once a player descriptor is threaded here (Phase 4
/// player-leave policy / spawn-point selection). Kept as an explicit enum so the
/// fallback is a named, auditable decision rather than an implicit default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotPawnSource {
    /// A `Transform`-only inert pawn created by `crate::netcode`. Carries no
    /// `PlayerMovementComponent` — it is server-authoritative and inert in Phase 2
    /// (no client gameplay input is applied; local prediction starts in Phase 3).
    TransformFixture,
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
) -> (EntityId, NetworkId) {
    // Idempotency: an accept for a slot that already owns a live pawn is a no-op
    // beyond returning the existing identity. Re-registering in the replicable set is
    // harmless (it is a set), and re-stamping the allocator returns the same stable
    // NetworkId for the same EntityId.
    if let Some(existing) = slot_pawns.pawns.get(&client_id).copied() {
        if registry.exists(existing) {
            let net_id = allocator.stamp(existing);
            replicable.register(existing);
            return (existing, net_id);
        }
        // The mapped pawn is stale (despawned elsewhere). Fall through and re-create.
        slot_pawns.pawns.remove(&client_id);
    }

    let pawn = match source {
        // Transform-only fixture: an inert pawn at the world origin. No
        // PlayerMovementComponent is materialized from the fallback (entity_model.md
        // §7b: movement is descriptor-owned; this is not a real movement pawn).
        SlotPawnSource::TransformFixture => registry.spawn(Transform::default()),
    };

    // Stamp the stable session-monotonic NetworkId and register for replication.
    let net_id = allocator.stamp(pawn);
    replicable.register(pawn);
    slot_pawns.pawns.insert(client_id, pawn);
    log::info!("[Net] slot {client_id} accepted: spawned inert remote pawn {pawn:?} as {net_id:?}");
    (pawn, net_id)
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
        let owned: Vec<EntitySnapshot> = produce_owned_snapshots(registry, replicable, allocator);
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
        );

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
        let owned = produce_owned_snapshots(&registry, &replicable, &mut allocator);
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
        );
        let _ = on_slot_accepted(
            &mut registry,
            &mut slot_pawns,
            &mut allocator,
            &mut replicable,
            CLIENT_B,
            SlotPawnSource::TransformFixture,
        );

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
        );
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
        );
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
        );
        let (pawn2, net2) = on_slot_accepted(
            &mut registry,
            &mut slot_pawns,
            &mut allocator,
            &mut replicable,
            CLIENT_A,
            SlotPawnSource::TransformFixture,
        );
        assert_eq!(pawn1, pawn2, "no duplicate pawn on re-accept");
        assert_eq!(net1.0, net2.0, "stable NetworkId on re-accept");
        assert_eq!(slot_pawns.len(), 1);
    }
}
