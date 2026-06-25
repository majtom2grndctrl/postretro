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

use crate::scripting::components::player_movement::PlayerMovementComponent;
use crate::scripting::registry::{ComponentKind, EntityId, EntityRegistry, Transform};

use super::descriptor_class::{descriptor_entity_class, is_networked_ai_map_enemy};
use super::movement_state::movement_state_to_wire;
use super::{
    HostCommandQueues, MovementOwners, NetworkIdAllocator, component_kind_discriminant,
    transform_to_wire,
};

/// The Phase 2 replicable set: entities `crate::netcode` has explicitly registered
/// as authoritative networked gameplay objects — slot-owned movement pawns, the
/// host's own pawn, and map-placed AI enemies (Brain + Agent from a `MapPlacement`
/// descriptor spawn). This set is the registration mechanism the predicate consults.
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
    // movement pawns and the host's own pawn, and the enemy sweep registers
    // map-placed AI enemies.
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
/// registered in [`ReplicableSet`] (slot-owned movement pawns, the host's own pawn,
/// and map-placed AI enemies — the authoritative networked gameplay objects
/// `crate::netcode` registers). The Phase 1 all-`Transform` walk is deliberately
/// *not* reused.
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
    owners: &MovementOwners,
    command_queues: &HostCommandQueues,
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
        // Movement-authority metadata (M15 Phase 3): a pawn owned by a client carries
        // its owner id + resolved command cursor. Unowned entities (the Transform-only
        // fixtures, the demo mover) carry neither — produced as an `unowned` snapshot.
        let owner_client_id = owners.owner_of(id);
        let last_processed_client_tick =
            owner_client_id.and_then(|cid| command_queues.resolved_cursor(cid));
        // Descriptor class the entity was materialized from (M15 Phase 3 Task 7 / E10
        // Task 4), so the recipient can materialize the matching descriptor-backed
        // component locally. Read from the entity's own `DescriptorProvenance`: a net-slot
        // movement pawn stamps `canonical_name` (the resolved `entity_class`, default
        // `"player"`); a map-placed AI enemy stamps its descriptor class on its
        // Transform-only record (the wire now allows `entity_class` on any non-despawn
        // finite-`Transform` record). A non-descriptor entity stays `None`.
        let entity_class = descriptor_entity_class(registry, id, &components);
        snapshots.push(EntitySnapshot {
            network_id,
            components,
            owner_client_id,
            last_processed_client_tick,
            entity_class,
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
    // Append the movement payload (M15 Phase 3) in stable order after Transform, when
    // the entity carries a live `PlayerMovementComponent` (a descriptor-backed net-slot
    // pawn). The Transform-only fixtures and the demo mover lack the component, so they
    // still emit Transform alone. `movement_state_to_wire` extracts only the mutable
    // tick subset; descriptor tuning stays local on both peers.
    if let Ok(movement) = registry.get_component::<PlayerMovementComponent>(id) {
        let payload = ComponentPayload::PlayerMovementState(movement_state_to_wire(movement));
        debug_assert_eq!(
            component_kind_discriminant(ComponentKind::PlayerMovement),
            payload.kind(),
            "engine/wire movement discriminant diverged"
        );
        payloads.push(payload);
    }
    payloads
}

/// Register the host's map-placed AI enemies for outbound replication (E10 Task 4):
/// every entity carrying `Brain` + `Agent` from a `MapPlacement` descriptor spawn
/// ([`is_networked_ai_map_enemy`]) enters the [`ReplicableSet`] and is stamped a stable
/// `NetworkId`, so its authoritative `Transform` replicates to clients. Static descriptor
/// props (a light/mesh/health placement without AI) stay unregistered.
///
/// Reload-safe and idempotent. `tracked` is the host endpoint's owning set of the
/// previously-registered enemy ids: on a level reload the freshly-spawned enemies are
/// distinct `EntityId`s, so every stale tracked id is unregistered (and dropped from
/// `tracked`) before the new sweep registers this level's enemies. Re-running the sweep
/// on the same level is a no-op (the set, the allocator, and `tracked` are all stable
/// per `EntityId`). The host pawn's own registration lives in `host_register_own_pawn`;
/// this is the enemy-only counterpart.
///
/// Host-gated by the caller (it only runs inside the `NetEndpoint::Host` arm). Reads the
/// registry through the borrow the caller threads in and touches only the replication
/// bookkeeping — it never reaches into `App`.
pub(crate) fn host_register_map_enemies(
    registry: &EntityRegistry,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    tracked: &mut HashSet<EntityId>,
) {
    // Drop any prior-level enemy registrations first. A reload bumps the registry
    // generation on the old ids, so each previously-tracked id either no longer exists
    // or is a different entity now — never the live enemy we are about to register.
    for stale in tracked.drain() {
        replicable.unregister(stale);
        // Prune the dead EntityId mapping so the allocator map does not accrue one
        // entry per ever-spawned enemy. NetworkIds stay monotonic; only the stale
        // mapping is dropped.
        allocator.forget(stale);
    }

    let mut count = 0usize;
    for (id, _) in registry.iter_with_kind(ComponentKind::Brain) {
        if !is_networked_ai_map_enemy(registry, id) {
            continue;
        }
        // Stamp the stable session-monotonic NetworkId and register for replication.
        // No `MovementOwners` entry: an AI enemy is host-authoritative and unowned by any
        // client, so its per-recipient `local_player` flag is false everywhere. Its class
        // rides the Transform-only snapshot via `descriptor_entity_class`.
        allocator.stamp(id);
        replicable.register(id);
        tracked.insert(id);
        count += 1;
    }
    if count > 0 {
        log::info!("[Net] host registered {count} map-placed AI enemy/enemies for replication");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use crate::scripting::components::agent::AgentComponent;
    use crate::scripting::components::brain::{AiStateMap, AiTuning, BrainComponent, LogicalState};
    use crate::scripting::components::health::HealthComponent;
    use crate::scripting::components::mesh::MeshComponent;
    use crate::scripting::provenance::{
        DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath,
    };
    use crate::scripting::registry::{ComponentValue, Transform};
    use glam::Vec3;

    // A minimal valid AI brain — the predicate only needs the component PRESENT, but a
    // real `BrainComponent` keeps the fixture honest about what an `ai` descriptor block
    // materializes.
    fn brain() -> BrainComponent {
        BrainComponent {
            state: LogicalState::Idle,
            attack_cooldown_remaining_ms: 0.0,
            think_stride_counter: 0,
            death_despawn_remaining_ms: None,
            tuning: AiTuning {
                detection_range: 18.0,
                attack_range: 2.0,
                leash_range: 26.0,
                attack_damage: 8.0,
                attack_cooldown_ms: 1000.0,
                move_speed: 3.5,
                death_despawn_ms: 1500.0,
                states: AiStateMap {
                    idle: "idle".into(),
                    alert: "locomotion".into(),
                    attack: "attack".into(),
                    death: "death".into(),
                },
            },
        }
    }

    fn agent() -> AgentComponent {
        AgentComponent::new(0.4, 1.6, 0.3, 3.5)
    }

    fn provenance(name: &str, spawn_path: DescriptorSpawnPath) -> DescriptorProvenance {
        DescriptorProvenance {
            canonical_name: name.to_string(),
            owned_components: std::iter::once(DescriptorComponentKind::Health).collect(),
            map_overrides: Default::default(),
            spawn_path,
        }
    }

    /// Spawn a map-placed AI enemy the way `apply_data_archetype_dispatch` does: a
    /// Transform, `Brain` + `Agent` from the `ai` block, and a `MapPlacement`
    /// `DescriptorProvenance` naming the descriptor class.
    fn spawn_ai_map_enemy(registry: &mut EntityRegistry, class: &str) -> EntityId {
        let id = registry.spawn(Transform {
            position: Vec3::new(5.0, 0.0, 0.0),
            ..Transform::default()
        });
        let _ = registry.set_component_value(id, ComponentValue::Brain(brain()));
        let _ = registry.set_component_value(id, ComponentValue::Agent(agent()));
        let _ = registry.set_component(id, provenance(class, DescriptorSpawnPath::MapPlacement));
        id
    }

    /// Spawn a static (non-AI) map-placed descriptor prop: a Transform, a health/mesh
    /// component, and a `MapPlacement` provenance — but NO `Brain`/`Agent`.
    fn spawn_static_descriptor_prop(registry: &mut EntityRegistry, class: &str) -> EntityId {
        let id = registry.spawn(Transform {
            position: Vec3::new(7.0, 0.0, 0.0),
            ..Transform::default()
        });
        let _ = registry.set_component(
            id,
            HealthComponent {
                max: 100.0,
                current: 100.0,
                hitbox: None,
                death_handled: false,
                zone_multipliers: Default::default(),
            },
        );
        let _ = registry.set_component(id, MeshComponent::stateless("barrel".into()));
        let _ = registry.set_component(id, provenance(class, DescriptorSpawnPath::MapPlacement));
        id
    }

    // E10 Task 4: the host registers a map-placed AI enemy (Brain + Agent + MapPlacement)
    // in the ReplicableSet and stamps it a NetworkId; the id is tracked for reload cleanup.
    #[test]
    fn host_registers_ai_map_enemy_and_stamps_network_id() {
        let mut registry = EntityRegistry::new();
        let enemy = spawn_ai_map_enemy(&mut registry, "grunt");

        let mut allocator = NetworkIdAllocator::new();
        let mut set = ReplicableSet::new();
        let mut tracked: HashSet<EntityId> = HashSet::new();
        host_register_map_enemies(&registry, &mut allocator, &mut set, &mut tracked);

        assert!(
            set.contains(enemy),
            "the AI map enemy is registered for replication"
        );
        assert!(
            tracked.contains(&enemy),
            "the enemy id is tracked in the host endpoint's set for reload cleanup"
        );
        // A NetworkId was stamped (stable on re-stamp).
        let net_id = allocator.stamp(enemy);
        assert_eq!(allocator.stamp(enemy), net_id, "stamped id is stable");
    }

    // E10 Task 4: a non-AI static descriptor prop (MapPlacement, no Brain/Agent) is NOT
    // registered — only AI enemies cross the wire from this path.
    #[test]
    fn host_does_not_register_static_descriptor_prop() {
        let mut registry = EntityRegistry::new();
        let prop = spawn_static_descriptor_prop(&mut registry, "barrel");

        let mut allocator = NetworkIdAllocator::new();
        let mut set = ReplicableSet::new();
        let mut tracked: HashSet<EntityId> = HashSet::new();
        host_register_map_enemies(&registry, &mut allocator, &mut set, &mut tracked);

        assert!(
            !set.contains(prop),
            "a static descriptor prop without Brain+Agent stays off the wire"
        );
        assert!(tracked.is_empty(), "no static prop is tracked");
    }

    // E10 Task 4 reload safety: a simulated level reload (despawn the old enemies, spawn
    // fresh ones) unregisters the stale ids before registering the new level's enemies —
    // no duplicate or leaked registration carries across the reload.
    #[test]
    fn host_reload_unregisters_stale_enemy_ids() {
        let mut registry = EntityRegistry::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut set = ReplicableSet::new();
        let mut tracked: HashSet<EntityId> = HashSet::new();

        // Level 1: one enemy registered + tracked.
        let level1_enemy = spawn_ai_map_enemy(&mut registry, "grunt");
        host_register_map_enemies(&registry, &mut allocator, &mut set, &mut tracked);
        assert!(set.contains(level1_enemy));

        // Reload: despawn the level-1 enemy and spawn a fresh level-2 enemy (a distinct
        // EntityId — the registry bumps the slot generation on despawn).
        registry.despawn(level1_enemy).expect("live enemy despawns");
        let level2_enemy = spawn_ai_map_enemy(&mut registry, "grunt");
        assert_ne!(
            level1_enemy, level2_enemy,
            "the reloaded enemy is a distinct entity"
        );

        host_register_map_enemies(&registry, &mut allocator, &mut set, &mut tracked);

        assert!(
            !set.contains(level1_enemy),
            "the stale level-1 id is unregistered on reload"
        );
        assert!(
            set.contains(level2_enemy),
            "the fresh level-2 enemy is registered"
        );
        assert_eq!(tracked.len(), 1, "exactly one enemy tracked after reload");
        assert!(tracked.contains(&level2_enemy));
    }

    // Fix A: reload cleanup also prunes the allocator's EntityId->NetworkId map so it
    // does not accrue a dead entry per ever-spawned enemy. NetworkIds stay monotonic —
    // the fresh enemy gets a new, higher id, never the dropped stale one.
    #[test]
    fn host_reload_forgets_dead_enemy_from_allocator_map() {
        let mut registry = EntityRegistry::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut set = ReplicableSet::new();
        let mut tracked: HashSet<EntityId> = HashSet::new();

        // Level 1: one enemy registered, stamped, and mapped in the allocator.
        let level1_enemy = spawn_ai_map_enemy(&mut registry, "grunt");
        host_register_map_enemies(&registry, &mut allocator, &mut set, &mut tracked);
        let level1_net_id = allocator.stamp(level1_enemy);
        assert!(
            allocator.maps_entity(level1_enemy),
            "the level-1 enemy is mapped in the allocator after registration"
        );

        // Reload: despawn the level-1 enemy and spawn a fresh, distinct level-2 enemy.
        registry.despawn(level1_enemy).expect("live enemy despawns");
        let level2_enemy = spawn_ai_map_enemy(&mut registry, "grunt");
        host_register_map_enemies(&registry, &mut allocator, &mut set, &mut tracked);

        assert!(
            !allocator.maps_entity(level1_enemy),
            "the dead level-1 EntityId is forgotten from the allocator map on reload"
        );

        // Monotonicity intact: the fresh enemy gets a new, higher NetworkId — the dropped
        // stale id is never recycled.
        let level2_net_id = allocator.stamp(level2_enemy);
        assert!(
            level2_net_id.0 > level1_net_id.0,
            "the reloaded enemy gets a new, higher NetworkId; ids are never recycled"
        );
    }

    // E10 Task 4: snapshot production stamps `entity_class` from DescriptorProvenance for
    // a registered map-placed AI enemy — its Transform-only record carries the class.
    #[test]
    fn producer_stamps_entity_class_for_registered_ai_enemy() {
        let mut registry = EntityRegistry::new();
        let enemy = spawn_ai_map_enemy(&mut registry, "grunt");

        let mut allocator = NetworkIdAllocator::new();
        let mut set = ReplicableSet::new();
        let mut tracked: HashSet<EntityId> = HashSet::new();
        host_register_map_enemies(&registry, &mut allocator, &mut set, &mut tracked);

        let snaps = produce_owned_snapshots(
            &registry,
            &set,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        let snap = snaps
            .iter()
            .find(|s| s.network_id == allocator.stamp(enemy).0)
            .expect("the registered enemy is produced");
        assert_eq!(
            snap.entity_class,
            Some("grunt".to_string()),
            "the enemy's snapshot carries its descriptor class"
        );
        // It rides a Transform-only record (no movement payload) and is unowned.
        assert!(
            snap.components
                .iter()
                .all(|c| matches!(c, ComponentPayload::Transform(_))),
            "an AI enemy replicates Transform-only"
        );
        assert_eq!(snap.owner_client_id, None, "an AI enemy is host-unowned");
    }

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

        let snaps = produce_owned_snapshots(
            &registry,
            &set,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
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
        let snaps2 = produce_owned_snapshots(
            &registry,
            &set,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
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
        let snaps = produce_owned_snapshots(
            &registry,
            &set,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        assert!(
            snaps.is_empty(),
            "a vanished registered entity is not produced"
        );
    }
}
