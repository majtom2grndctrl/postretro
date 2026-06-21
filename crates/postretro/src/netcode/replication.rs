// Engine-side Phase 2 replication glue: the replicable-set registration + predicate
// and the owned post-tick snapshot producer that feeds `postretro-net`'s
// `ServerReplication`.
// See: context/lib/networking.md
//
// This is the engine half of M15 Phase 2 server replication. It owns the
// `EntityId <-> NetworkId` bridge for replication: the net crate is registry-blind
// and keyed only by `NetworkId`, so this module decides *which* entities replicate
// (the Phase 2 replicable-set predicate) and copies their state into owned wire
// mirrors keyed by `NetworkId`, then hands those to the net tracker.
//
// Owned post-tick rule: borrow the registry once, copy replicable state into owned
// `EntitySnapshot`s (releasing the borrow), then hand them to the net crate for
// per-client encoding. The registry is never held across the net-crate call.

use std::collections::HashSet;

use postretro_net::replication::EntitySnapshot;
use postretro_net::wire::ComponentPayload;

use crate::scripting::registry::{ComponentKind, EntityId, EntityRegistry, Transform};

use super::{NetworkIdAllocator, component_kind_discriminant, transform_to_wire};

/// The Phase 2 replicable set: entities `crate::netcode` has explicitly registered
/// as authoritative networked gameplay objects. Task 4 registers the slot-owned
/// inert pawns and Task 6 the host-owned demo mover; this set is the registration
/// mechanism the predicate consults.
///
/// Membership is by `EntityId`. The predicate ([`is_replicable`]) is the authority
/// on what crosses the wire — this set is its allow-list, layered over the
/// component-kind exclusions below. An entity not in this set does not replicate,
/// even if it carries a `Transform` (the Phase 1 all-`Transform` walk is gone).
#[derive(Debug, Default)]
pub(crate) struct ReplicableSet {
    registered: HashSet<EntityId>,
}

impl ReplicableSet {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    // The register/unregister/contains surface is the registration mechanism for
    // authoritative networked entities: the lifecycle glue registers slot-owned
    // inert pawns and the demo path registers the host-owned mover.
    /// Register an entity as an authoritative networked gameplay object. Idempotent.
    pub(crate) fn register(&mut self, id: EntityId) {
        self.registered.insert(id);
    }

    /// Stop replicating an entity (e.g. it despawned in game logic). Idempotent.
    pub(crate) fn unregister(&mut self, id: EntityId) {
        self.registered.remove(&id);
    }

    /// Membership test. Only the `is_replicable` predicate and lifecycle tests
    /// consult it directly; `produce_owned_snapshots` walks `iter` instead.
    #[cfg(test)]
    pub(crate) fn contains(&self, id: EntityId) -> bool {
        self.registered.contains(&id)
    }

    /// Iterate registered entity ids. Order is unspecified (a `HashSet`); the net
    /// tracker keys by `NetworkId` and does not depend on entity order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.registered.iter().copied()
    }
}

/// Phase 2 replicable-set predicate. An entity replicates iff it is explicitly
/// registered in [`ReplicableSet`] (the host-owned demo mover, slot-owned inert
/// pawns, and entities `crate::netcode` registered as authoritative networked
/// gameplay objects). The Phase 1 all-`Transform` walk is deliberately *not* reused.
///
/// Registration is the allow-list, so deterministic client-local / baked
/// presentation entities (`BillboardEmitter`, `ParticleState`, `SpriteVisual`,
/// `Light`, `FogVolume`) and ordinary static map transforms stay off the wire by
/// default — they are simply never registered. The exclusion is also enforced
/// structurally on the payload side: [`collect_payloads`] only pulls authoritative
/// gameplay components (`Transform`; the movement subset later), never the
/// presentation kinds, so a registered entity never leaks a baked/cosmetic payload.
///
/// `produce_owned_snapshots` consults the set directly via `iter`; this standalone
/// single-entity predicate is exercised only by this module's tests.
#[cfg(test)]
pub(crate) fn is_replicable(set: &ReplicableSet, id: EntityId) -> bool {
    set.contains(id)
}

/// Produce the owned post-tick snapshots for the net tracker. Borrows the registry
/// immutably, copies each replicable entity's wire-mirror state into an owned
/// [`EntitySnapshot`] keyed by `NetworkId`, then returns — the caller releases the
/// borrow before handing these to `postretro-net`.
///
/// Stamps each replicable `EntityId` to its stable `NetworkId` via the allocator.
/// Only registered entities are produced; the component order per entity is stable
/// (`Transform` then `PlayerMovementState`) so the net crate's wire-mirror equality
/// dirty-check is order-stable.
pub(crate) fn produce_owned_snapshots(
    registry: &EntityRegistry,
    set: &ReplicableSet,
    allocator: &mut NetworkIdAllocator,
) -> Vec<EntitySnapshot> {
    let mut snapshots = Vec::new();
    for id in set.iter() {
        if !registry.exists(id) {
            // A registered-but-vanished entity: skip. The net tracker sees it absent
            // from this tick and despawns it (the registration cleanup is the game
            // logic's job; the predicate just does not produce a payload).
            continue;
        }
        let components = collect_payloads(registry, id);
        let network_id = allocator.stamp(id).0;
        snapshots.push(EntitySnapshot {
            network_id,
            components,
        });
    }
    snapshots
}

/// Collect the wire-mirror payloads for one replicable entity, in a stable order:
/// `Transform` first, then `PlayerMovementState` if present. Excluded presentation
/// components are never collected. Returns an empty vec if the entity carries no
/// replicable component (the entity still appears in the snapshot so the tracker
/// can track its lifecycle, but with no payload).
fn collect_payloads(registry: &EntityRegistry, id: EntityId) -> Vec<ComponentPayload> {
    let mut payloads = Vec::new();
    if let Ok(transform) = registry.get_component::<Transform>(id) {
        // The collection deliberately skips presentation kinds entirely by only
        // pulling the wire-bound gameplay components (Transform today; the movement
        // subset is added when a replicated entity carries a live
        // PlayerMovementComponent — see below).
        let payload = ComponentPayload::Transform(transform_to_wire(transform));
        // Live cross-check of the engine->wire discriminant mapping (the drift-guard
        // tests pin it both sides; a divergence would mis-tag replication).
        debug_assert_eq!(
            component_kind_discriminant(ComponentKind::Transform),
            payload.kind(),
            "engine/wire component discriminant diverged"
        );
        payloads.push(payload);
    }
    // No PlayerMovementState payload is emitted today: the Phase 2 host-owned demo
    // mover is Transform-only, and `WirePlayerMovementState` is only meaningfully
    // assembled from an entity that already has a live `PlayerMovementComponent`,
    // which the Phase 2 fixture lacks. When a host-authoritative entity carries that
    // component, append its movement payload here in stable order (after Transform).
    payloads
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::registry::Transform;
    use glam::Vec3;

    // The predicate gates strictly on registration: an unregistered Transform-only
    // entity (an ordinary static map transform) does NOT replicate; registering it
    // (the test fixture exercising the path) makes it replicable.
    #[test]
    fn predicate_replicates_only_registered_entities() {
        let mut registry = EntityRegistry::new();
        let unregistered = registry.spawn(Transform::default());
        let registered = registry.spawn(Transform {
            position: Vec3::new(3.0, 0.0, 0.0),
            ..Transform::default()
        });
        let mut set = ReplicableSet::new();
        set.register(registered);

        assert!(
            !is_replicable(&set, unregistered),
            "an unregistered Transform-only entity stays off the wire"
        );
        assert!(
            is_replicable(&set, registered),
            "a registered entity replicates"
        );
    }

    // The owned-snapshot producer stamps stable NetworkIds and copies only
    // registered entities into owned snapshots keyed by NetworkId.
    #[test]
    fn producer_emits_only_registered_entities_with_stable_ids() {
        let mut registry = EntityRegistry::new();
        let a = registry.spawn(Transform {
            position: Vec3::new(1.0, 0.0, 0.0),
            ..Transform::default()
        });
        let _ignored = registry.spawn(Transform {
            position: Vec3::new(2.0, 0.0, 0.0),
            ..Transform::default()
        });
        let mut set = ReplicableSet::new();
        set.register(a);
        let mut allocator = NetworkIdAllocator::new();

        let snaps = produce_owned_snapshots(&registry, &set, &mut allocator);
        assert_eq!(snaps.len(), 1, "only the registered entity is produced");
        let net_id = allocator.stamp(a).0;
        assert_eq!(
            snaps[0].network_id, net_id,
            "stamped with its stable NetworkId"
        );
        assert_eq!(
            snaps[0].components.len(),
            1,
            "carries its Transform payload"
        );
        assert!(matches!(
            snaps[0].components[0],
            ComponentPayload::Transform(_)
        ));

        // A second pass yields the same NetworkId for the same EntityId.
        let snaps2 = produce_owned_snapshots(&registry, &set, &mut allocator);
        assert_eq!(
            snaps2[0].network_id, net_id,
            "NetworkId stable across ticks"
        );
    }

    // A registered entity that vanished from the registry is skipped (not produced),
    // so the net tracker sees it absent and despawns it.
    #[test]
    fn producer_skips_registered_but_despawned_entity() {
        let mut registry = EntityRegistry::new();
        let a = registry.spawn(Transform::default());
        let mut set = ReplicableSet::new();
        set.register(a);
        let mut allocator = NetworkIdAllocator::new();

        // Despawn the entity in game logic but leave it registered (the producer
        // tolerates the lag). `despawn` returns a Result; the id is live here.
        registry.despawn(a).expect("live entity despawns");
        let snaps = produce_owned_snapshots(&registry, &set, &mut allocator);
        assert!(
            snaps.is_empty(),
            "a vanished registered entity is not produced"
        );
    }
}
