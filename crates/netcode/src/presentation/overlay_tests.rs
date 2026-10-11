//! End-to-end CPU proof for the shared damaged-enemy roster. Damage enters
//! through the health dispatcher, facts cross the real participation transport,
//! and client projections use actual snapshot-created entity identities.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use super::*;
use crate::client::NoPresentationBindings;
use crate::presentation::ingest_client_presentation_messages;
use crate::replication::{host_register_descriptor_entity, produce_owned_snapshots};
use crate::{HostCommandQueues, MovementOwners, NetworkIdAllocator};
use postretro_entities::components::health::{
    DamageContext, DamageProducer, apply_damage_with_context,
};
use postretro_entities::presentation_overrides::PresentationOverrides;
use postretro_entities::provenance::{
    DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath,
};
use postretro_entities::{EntityStateComponent, PresentationFact, ScriptCtx, Transform};
use postretro_foundation::{DamagePayload, ImpactEventDescriptor};
use postretro_net::replication::ServerReplication;
use postretro_net::transport::NetClient;
use postretro_net::wire::{self, SnapshotMessage};
use postretro_scripting_core::data_descriptors::{
    HealthDescriptor, HitboxDescriptor, PresentationOverlay, PresentationTemplate,
};
use serde_json::json;

const CLASS: &str = "test.overlay-target";
const EPSILON: f32 = 1.0e-5;

fn approx(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= EPSILON,
        "{actual} != {expected}"
    );
}

fn overlay_template() -> PresentationTemplate {
    serde_json::from_value(json!({
        "id": "test.enemy-status",
        "root": { "kind": "text", "content": "0", "fontSize": 16.0,
            "color": [1.0, 1.0, 1.0, 1.0] },
        "worldAnchor": { "socket": "status", "offsetY": 0.25 },
        "lifetimeMs": 750,
        "motion": { "rise": 0.0, "easing": "easeOut" },
        "fade": { "startMs": 500 }, "spawnScatter": { "radius": 0.0 }
    }))
    .unwrap()
}

fn descriptor() -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        faction: None,
        tolerance: None,
        canonical_name: Some(CLASS.to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: None,
        mesh: None,
        behavior: None,
        health: Some(HealthDescriptor {
            max: 100.0,
            hitbox: Some(HitboxDescriptor {
                half_extents: [0.5, 1.5, 0.5],
                offset: Some([0.0, 0.25, 0.0]),
            }),
            zone_multipliers: Default::default(),
        }),
    }
}

struct Host {
    ctx: ScriptCtx,
    runtime: ImpactPolicyRuntime,
    tracker: HostOverlayFactTracker,
    pool: PresentationPool,
    replicable: ReplicableSet,
    allocator: NetworkIdAllocator,
    tracked: std::collections::HashSet<EntityId>,
    overrides: PresentationOverrides,
    zones: HitZoneStore,
    server: NetServer,
    address: SocketAddr,
}

impl Host {
    fn new(cap: usize, linger_ms: u32) -> Self {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = socket.local_addr().unwrap();
        let mut server = NetServer::new(socket, address, 8, Duration::from_secs(1), None).unwrap();
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        server.set_mod_digest(Some([7; 32]));
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        let ctx = ScriptCtx::new();
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_presentation_templates(vec![overlay_template()]);
        let overlay: PresentationOverlay = serde_json::from_value(json!({
            "over": { "kind": "damagedEnemies", "lingerMs": linger_ms,
                "hideAtFull": true,
                "shield": {
                    "value": { "op": "input", "name": "@state.shield" },
                    "max": { "op": "input", "name": "@state.shieldMax" }
                } },
            "template": "test.enemy-status", "maxVisible": cap
        }))
        .unwrap();
        runtime.replace_presentation_overlays(vec![overlay]);
        // This consequence deliberately restores the final health fraction on
        // every hit. Refresh therefore cannot be inferred from fraction changes.
        runtime.replace_global_events(vec![ImpactEventDescriptor {
            id: "restore-final-health".to_string(),
            is_override: false,
            levels: Vec::new(),
            filter_tag: Some("restore".to_string()),
            policy: vec![
                json!({ "primitive": "setHealth", "target": "@impact.target",
                "args": { "value": { "op": "const", "value": 75.0 } } }),
            ],
        }]);
        assert!(
            runtime.client_overlay_config().is_some(),
            "real overlay declaration binds"
        );
        Self {
            ctx,
            runtime,
            tracker: HostOverlayFactTracker::default(),
            pool: PresentationPool::new(1),
            replicable: ReplicableSet::new(),
            allocator: NetworkIdAllocator::new(),
            tracked: Default::default(),
            overrides: PresentationOverrides::default(),
            zones: HitZoneStore::new(),
            server,
            address,
        }
    }

    fn spawn_target(&mut self, restore: bool) -> EntityId {
        let mut registry = self.ctx.registry.borrow_mut();
        let entity = registry.spawn(Transform::default());
        registry
            .set_component(
                entity,
                HealthComponent::from_descriptor(&descriptor().health.unwrap()),
            )
            .unwrap();
        let mut state = EntityStateComponent::default();
        state.set("shield", 25.0);
        state.set("shieldMax", 50.0);
        registry.set_component(entity, state).unwrap();
        registry
            .set_tags(
                entity,
                if restore {
                    vec!["restore".to_string()]
                } else {
                    Vec::new()
                },
            )
            .unwrap();
        registry
            .set_component(
                entity,
                DescriptorProvenance {
                    canonical_name: CLASS.to_string(),
                    owned_components: [DescriptorComponentKind::Health].into_iter().collect(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::MapPlacement,
                },
            )
            .unwrap();
        entity
    }

    fn target(&mut self, restore: bool) -> EntityId {
        let entity = self.spawn_target(restore);
        assert!(host_register_descriptor_entity(
            &self.ctx.registry.borrow(),
            &mut self.allocator,
            &mut self.replicable,
            &mut self.tracked,
            &mut self.overrides,
            entity
        ));
        entity
    }

    fn id(&self, entity: EntityId) -> NetworkId {
        self.allocator.network_id_for_entity(entity).unwrap()
    }

    fn hit(&mut self, entity: EntityId, source: Option<EntityId>, time: f64, amount: f32) {
        self.ctx.presentation_script_time.set(time);
        let config = self.runtime.client_overlay_config().unwrap();
        let mut registry = self.ctx.registry.borrow_mut();
        let mut context = DamageContext::new("test-overlay", DamageProducer::InTick);
        context.attacker = source;
        apply_damage_with_context(
            &mut registry,
            entity,
            &DamagePayload {
                amount,
                impulse: Vec3::ZERO,
            },
            context,
        );
        self.runtime.evaluate_pending_in_registry_with_overlay_sink(
            &mut registry,
            |registry, damage| {
                host_register_descriptor_entity(
                    registry,
                    &mut self.allocator,
                    &mut self.replicable,
                    &mut self.tracked,
                    &mut self.overrides,
                    damage.entity,
                );
                let id = self.allocator.network_id_for_entity(damage.entity).unwrap();
                self.tracker.record_damage(
                    registry,
                    id,
                    damage,
                    config.linger_seconds,
                    config.max_visible,
                );
                assert!(self.tracker.live.len() <= config.max_visible);
            },
        );
        assert!(
            self.runtime.take_damaged_enemy_overlay_damage().is_empty(),
            "host sink leaves no source backlog"
        );
        assert!(
            registry.take_impact_dispatches().is_empty(),
            "damage dispatcher drains synchronously"
        );
    }

    fn frame(&mut self, time: f64) {
        update_host_overlay_roster(
            &mut self.tracker,
            &mut self.runtime,
            &self.ctx.registry.borrow(),
            &self.replicable,
            &mut self.pool,
            &self.zones,
            &mut self.server,
            time,
            time,
        );
        let cap = self.runtime.client_overlay_config().unwrap().max_visible;
        assert!(self.tracker.live.len() <= cap);
        assert!(self.tracker.roster.entries.len() <= cap);
        assert!(self.tracker.scratch.len() <= cap);
        assert!(self.pool.live_counts().1 <= cap);
    }

    fn packets(&mut self, peer: &Peer) -> Vec<Vec<u8>> {
        self.server.packets_to_send(peer.id)
    }

    fn snapshot(&mut self, peer: &mut Peer, sequence: u32, host_time: f64, local_time: f64) {
        // Derive the baseline from the registry-owning production collector,
        // then use the actual net tracker, codec and framed Snapshot channel.
        let snapshots = produce_owned_snapshots(
            &self.ctx.registry.borrow(),
            &self.replicable,
            &mut self.allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        let mut tracker = ServerReplication::new();
        tracker.register_client(peer.id);
        tracker.ingest_tick(snapshots);
        let mut raw = tracker.encode_for_client(peer.id, sequence).unwrap();
        raw.sequence = sequence;
        raw.server_script_time = host_time;
        assert!(self.server.send_snapshot(peer.id, wire::encode(&raw)));
        peer.deliver(self.packets(peer));
        let received = peer.net.drain_snapshots();
        assert_eq!(received.len(), 1);
        let snapshot = crate::decode_snapshot(&received[0]).unwrap();
        peer.apply_snapshot(&snapshot, local_time);
    }
}

struct Peer {
    id: u64,
    net: NetClient,
    registry: EntityRegistry,
    replication: ClientReplication,
    state: ClientOverlayFactState,
    pool: PresentationPool,
}

impl Peer {
    fn connect(host: &mut Host, id: u64) -> Self {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut net =
            NetClient::new(socket, host.address, id, Duration::from_secs(1), None, None).unwrap();
        net.set_mod_identity("postretro.test".to_string(), "1".to_string());
        net.set_mod_digest(Some([7; 32]));
        net.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        postretro_net::harness::reveal_both(&mut host.server, &mut net, "test-level");
        host.server.add_relay_connection(id, None);
        net.set_connected();
        for _ in 0..8 {
            net.update_connections(Duration::from_millis(16));
            for packet in net.packets_to_send() {
                host.server.process_packet_from(&packet, id);
            }
            let _ = host.server.poll_handshakes();
            if host.server.is_participating(id) {
                break;
            }
        }
        assert!(host.server.is_participating(id));
        for packet in host.server.packets_to_send(id) {
            net.process_packet(&packet);
        }
        let _ = net.drain_control();
        assert!(net.is_participating());
        // Complete reliable marker acknowledgement before measuring cosmetics.
        net.update_connections(Duration::from_millis(16));
        for packet in net.packets_to_send() {
            host.server.process_packet_from(&packet, id);
        }
        let _ = host.server.packets_to_send(id);
        Self {
            id,
            net,
            registry: EntityRegistry::new(),
            replication: ClientReplication::new(),
            state: ClientOverlayFactState::default(),
            pool: PresentationPool::new(1),
        }
    }

    fn deliver(&mut self, packets: Vec<Vec<u8>>) {
        for packet in packets {
            self.net.process_packet(&packet);
        }
        let _ = self.net.drain_control();
    }

    fn apply_snapshot(&mut self, snapshot: &SnapshotMessage, local_time: f64) {
        let outcome = self.replication.apply_snapshot_with_presentation(
            &mut self.registry,
            snapshot,
            snapshot.server_tick,
            1.0 / 60.0,
            &mut NoPresentationBindings,
            local_time,
        );
        crate::snapshot_apply::materialize_remote_entities(
            &outcome.remote_entities,
            &[descriptor()],
            &mut self.registry,
            &mut self.replication,
            snapshot.server_tick,
            None,
            &HitZoneStore::new(),
        );
    }

    fn intake(&mut self, config: &ClientOverlayConfig, local_time: f64, rtt: Option<f64>) -> usize {
        let messages = self.net.drain_presentation();
        let count = messages.len();
        ingest_client_presentation_messages(
            &mut self.registry,
            messages,
            &[descriptor()],
            &HashMap::new(),
            &mut self.state,
            &self.replication,
            &mut self.pool,
            Some(config),
            &HitZoneStore::new(),
            None,
            local_time,
            local_time,
            rtt,
        );
        self.assert_bounds(config.max_visible);
        count
    }

    fn project(&mut self, config: &ClientOverlayConfig, local_time: f64, rtt: Option<f64>) {
        update_client_overlay_anchors(
            &self.registry,
            &[descriptor()],
            &mut self.state,
            &self.replication,
            &mut self.pool,
            Some(config),
            &HitZoneStore::new(),
            None,
            local_time,
            local_time,
            rtt,
        );
        self.assert_bounds(config.max_visible);
    }

    fn assert_bounds(&self, cap: usize) {
        assert!(
            self.state
                .roster
                .as_ref()
                .map_or(0, |roster| roster.entries.len())
                <= cap
        );
        assert!(self.state.projected.len() <= cap);
        assert!(self.pool.live_counts().1 <= cap);
    }

    fn ids(&self) -> Vec<NetworkId> {
        let mut ids: Vec<_> = self
            .pool
            .tracked_overlay_ids_iter()
            .map(|entity| self.replication.network_id_for_entity(entity).unwrap())
            .collect();
        ids.sort_by_key(|id| id.0);
        ids
    }

    fn facts(&self, id: NetworkId) -> &std::collections::BTreeMap<String, PresentationFact> {
        let entity = self.replication.entity_for_network_id(id).unwrap();
        self.pool.overlay_facts(entity).unwrap()
    }
}

fn number(facts: &std::collections::BTreeMap<String, PresentationFact>, key: &str) -> f32 {
    match facts.get(key).unwrap() {
        PresentationFact::Number(value) => *value,
        _ => panic!("numeric overlay fact"),
    }
}

#[test]
fn shared_roster_streams_all_attackers_through_transport_baselines_and_pools() {
    let mut host = Host::new(8, 2_000);
    let mut one = Peer::connect(&mut host, 41);
    let mut two = Peer::connect(&mut host, 73);
    let sources: Vec<_> = (0..4)
        .map(|_| host.ctx.registry.borrow_mut().spawn(Transform::default()))
        .collect();
    let mut owners = MovementOwners::new();
    owners.set(sources[1], one.id);
    owners.set(sources[2], two.id);
    assert_eq!(owners.owner_of(sources[0]), None, "listen-host source");
    assert_eq!(owners.owner_of(sources[1]), Some(one.id));
    assert_eq!(owners.owner_of(sources[2]), Some(two.id));
    // Fourth source models a host-owned NPC; fifth impact has no source.
    let targets: Vec<_> = (0..5).map(|_| host.target(false)).collect();
    for (index, (&target, source)) in targets
        .iter()
        .zip(sources.iter().copied().map(Some).chain([None]))
        .enumerate()
    {
        host.hit(target, source, 2.0, 10.0 + index as f32);
    }
    host.frame(2.0);
    let config = host.runtime.client_overlay_config().unwrap();
    let ids: Vec<_> = targets.iter().map(|&target| host.id(target)).collect();
    for peer in [&mut one, &mut two] {
        peer.deliver(host.packets(peer));
        assert_eq!(peer.intake(&config, 20.0, Some(0.0)), 1);
        assert!(
            peer.ids().is_empty(),
            "roster alone creates no entity or health authority"
        );
        assert_eq!(peer.state.pending_len(), 5);
        host.snapshot(peer, 1, 2.0, 20.0);
        peer.project(&config, 20.0, Some(0.0));
        assert_eq!(peer.ids(), ids);
        for (index, &id) in ids.iter().enumerate() {
            let facts = peer.facts(id);
            approx(
                number(facts, "healthFraction"),
                (90.0 - index as f32) / 100.0,
            );
            approx(number(facts, "shieldFraction"), 0.5);
            assert_eq!(facts.get("hasShield"), Some(&PresentationFact::Bool(true)));
            let entity = peer.replication.entity_for_network_id(id).unwrap();
            assert!(
                peer.registry
                    .get_component::<HealthComponent>(entity)
                    .is_err()
            );
            let anchor = peer.pool.overlay_anchor(entity).unwrap();
            approx(anchor.y, 2.0);
        }
    }
    for (index, &target) in targets.iter().enumerate() {
        approx(
            number(host.pool.overlay_facts(target).unwrap(), "healthFraction"),
            (90.0 - index as f32) / 100.0,
        );
    }
}

#[test]
fn newly_spawned_target_damaged_before_registration_sweep_joins_the_shared_roster() {
    let mut host = Host::new(1, 1_000);
    let mut peer = Peer::connect(&mut host, 41);
    let npc_source = host.ctx.registry.borrow_mut().spawn(Transform::default());
    let target = host.spawn_target(false);
    assert!(!host.replicable.contains(target));
    assert!(host.allocator.network_id_for_entity(target).is_none());
    host.hit(target, Some(npc_source), 1.0, 25.0);
    assert!(host.replicable.contains(target));
    assert_eq!(
        host.id(target),
        NetworkId(0),
        "fresh target can have id zero"
    );
    host.frame(1.0);
    let config = host.runtime.client_overlay_config().unwrap();
    peer.deliver(host.packets(&peer));
    assert_eq!(peer.intake(&config, 0.0, Some(0.0)), 1);
    host.snapshot(&mut peer, 1, 1.0, 0.0);
    peer.project(&config, 0.0, Some(0.0));
    assert_eq!(peer.ids(), [host.id(target)]);
    approx(number(peer.facts(host.id(target)), "healthFraction"), 0.75);
}

#[test]
fn equal_time_overflow_selects_stable_ids_without_lethal_displacement_and_refreshes_expiry() {
    let mut host = Host::new(2, 1_000);
    let mut peer = Peer::connect(&mut host, 41);
    let first = host.target(true);
    let second = host.target(true);
    let third = host.target(true);
    let doomed = host.target(false);
    // Arrival order intentionally disagrees with stable NetworkId order.
    for target in [third, first, second] {
        host.hit(target, None, 4.0, 1.0);
    }
    host.hit(doomed, None, 4.0, 100.0);
    host.frame(4.0);
    let expected = vec![host.id(second), host.id(third)];
    assert_eq!(
        host.tracker
            .roster
            .entries
            .iter()
            .map(|entry| entry.enemy_id)
            .collect::<Vec<_>>(),
        expected
    );
    let config = host.runtime.client_overlay_config().unwrap();
    peer.deliver(host.packets(&peer));
    peer.intake(&config, 0.0, Some(0.0));
    host.snapshot(&mut peer, 1, 4.0, 0.0);
    peer.project(&config, 0.0, Some(0.0));
    assert_eq!(peer.ids(), expected);
    let before = host.tracker.roster.revision;
    host.hit(second, None, 4.4, 1.0);
    // Host rendering is skipped after the damage. Original dispatch time owns
    // expiry; final sampling at 4.8 must not extend it to 5.8.
    host.frame(4.8);
    let refreshed = host
        .tracker
        .roster
        .entries
        .iter()
        .find(|entry| entry.enemy_id == host.id(second))
        .unwrap();
    assert!((refreshed.damaged_at - 4.4).abs() < 1.0e-9);
    assert!((refreshed.expires_at - 5.4).abs() < 1.0e-9);
    approx(refreshed.health_fraction, 0.75);
    assert!(
        host.tracker.roster.revision > before,
        "equal final health still refreshes expiry"
    );
    peer.deliver(host.packets(&peer));
    peer.intake(&config, 0.8, Some(0.0));
    peer.project(&config, 1.1, Some(0.0));
    assert_eq!(peer.ids(), [host.id(second)]);
    peer.project(&config, 1.41, Some(0.0));
    assert!(peer.ids().is_empty());
}

#[test]
fn lost_updates_repair_without_duplicate_refresh_and_new_empty_roster_beats_old_state() {
    let mut host = Host::new(2, 2_000);
    let mut peer = Peer::connect(&mut host, 41);
    let target = host.target(false);
    host.hit(target, None, 1.0, 10.0);
    host.frame(1.0);
    let original_roster = host.tracker.roster.clone();
    let old_packets = host.packets(&peer);
    assert!(!old_packets.is_empty()); // Drop the entire first publication.
    let config = host.runtime.client_overlay_config().unwrap();
    host.snapshot(&mut peer, 1, 1.0, 0.0);
    host.frame(1.24);
    peer.deliver(host.packets(&peer));
    assert!(
        peer.net.drain_presentation().is_empty(),
        "unchanged state has no early repair"
    );
    host.frame(1.25);
    let repair = host.packets(&peer);
    peer.deliver(repair.clone());
    assert_eq!(peer.intake(&config, 0.25, Some(0.0)), 1);
    assert_eq!(peer.ids(), [host.id(target)]);
    peer.deliver(repair);
    peer.intake(&config, 0.5, Some(0.0));
    assert_eq!(peer.ids(), [host.id(target)]);
    // Final frame removal precedes roster sampling, even without another hit.
    host.ctx.registry.borrow_mut().despawn(target).unwrap();
    host.replicable.unregister(target);
    host.frame(1.6);
    assert_eq!(host.pool.live_counts().1, 0);
    peer.deliver(host.packets(&peer));
    assert_eq!(peer.intake(&config, 0.6, Some(0.0)), 1);
    assert!(peer.ids().is_empty());
    peer.deliver(old_packets); // Delayed older revision cannot resurrect it.
    peer.intake(&config, 0.7, Some(0.0));
    assert!(peer.ids().is_empty());
    // Put the actual original producer output in a fresh transport packet as
    // well, so application revision ordering is proven independently of any
    // Renet duplicate-packet rejection.
    assert_eq!(
        host.server
            .broadcast_presentation(ServerPresentationMessage {
                payload: ServerPresentationPayload::OverlayRoster(original_roster),
            }),
        1
    );
    peer.deliver(host.packets(&peer));
    assert_eq!(peer.intake(&config, 0.7, Some(0.0)), 1);
    assert!(peer.ids().is_empty());
    assert!(peer.state.roster.as_ref().unwrap().entries.is_empty());
    assert!(peer.state.projected.is_empty());
    host.frame(1.85);
    peer.deliver(host.packets(&peer));
    assert_eq!(
        peer.intake(&config, 0.85, Some(0.0)),
        1,
        "empty terminal state also repairs"
    );
    assert!(peer.ids().is_empty());
}

#[test]
fn prebaseline_state_survives_old_ttl_but_terminal_before_baseline_never_projects() {
    let mut host = Host::new(2, 2_000);
    let mut peer = Peer::connect(&mut host, 41);
    let live = host.target(false);
    let dead = host.target(false);
    host.hit(live, None, 3.0, 10.0);
    host.hit(dead, None, 3.0, 10.0);
    host.frame(3.0);
    let config = host.runtime.client_overlay_config().unwrap();
    peer.deliver(host.packets(&peer));
    peer.intake(&config, 10.0, None);
    assert_eq!(peer.state.pending_len(), 2);
    host.hit(dead, None, 3.2, 100.0);
    host.frame(3.2);
    peer.deliver(host.packets(&peer));
    peer.intake(&config, 10.2, None);
    assert_eq!(peer.state.pending_len(), 1);
    // Baseline delay exceeds the old 0.5s pending-fact timeout. The original
    // live expiry is still 5.0, so clock initialization at 3.9 must project it.
    host.snapshot(&mut peer, 1, 3.9, 10.9);
    peer.project(&config, 10.9, None);
    assert!(
        peer.ids().is_empty(),
        "uninitialized RTT cannot guess zero age"
    );
    peer.project(&config, 10.9, Some(200_000.0));
    assert_eq!(peer.ids(), [host.id(live)]);
    peer.project(&config, 11.91, Some(200_000.0));
    assert!(
        peer.ids().is_empty(),
        "expiry comes from original host time"
    );
}

#[test]
fn synchronized_time_highwater_prevents_stale_snapshot_or_repair_resurrection_after_stall() {
    let mut host = Host::new(1, 1_000);
    let mut peer = Peer::connect(&mut host, 41);
    let target = host.target(false);
    host.hit(target, None, 4.0, 1.0);
    host.frame(4.0);
    let config = host.runtime.client_overlay_config().unwrap();
    let original = host.tracker.roster.clone();
    peer.deliver(host.packets(&peer));
    peer.intake(&config, 10.0, Some(0.0));
    host.snapshot(&mut peer, 2, 4.0, 10.0);
    peer.project(&config, 10.0, Some(0.0));
    assert_eq!(peer.ids(), [host.id(target)]);
    // No snapshots or rendering for > linger; the next frame retires it.
    peer.project(&config, 11.2, Some(0.0));
    assert!(peer.ids().is_empty());
    // A later accepted snapshot can still carry a regressed host clock after a
    // host stall. Equal-state repair plus RTT correction cannot rewind expiry.
    host.snapshot(&mut peer, 3, 4.2, 11.2);
    assert!(!peer.state.receive(original.clone(), config.max_visible));
    peer.project(&config, 11.2, Some(0.0));
    assert!(peer.ids().is_empty());
    let stale = SnapshotMessage {
        sequence: 1,
        server_tick: 1,
        server_script_time: 4.0,
        records: Vec::new(),
        state_schema_fingerprint: [0; 32],
        state_records: Vec::new(),
        faction_sentiment_record: None,
    };
    peer.apply_snapshot(&stale, 11.3);
    peer.state.receive(original, config.max_visible);
    peer.project(&config, 11.3, Some(0.0));
    assert!(peer.ids().is_empty());
    assert!(peer.state.roster.as_ref().unwrap().entries.is_empty());
}

// Regression: refreshed fractions and moving anchors must not become damage
// edges that restart the original host-authored linger.
#[test]
fn recharge_updates_fractions_and_moving_client_anchor_without_extending_expiry() {
    let mut host = Host::new(1, 2_000);
    let mut peer = Peer::connect(&mut host, 41);
    let target = host.target(false);
    let id = host.id(target);
    host.hit(target, None, 1.0, 30.0);
    host.frame(1.0);
    let original = host.tracker.roster.entries[0];
    let config = host.runtime.client_overlay_config().unwrap();
    peer.deliver(host.packets(&peer));
    assert_eq!(peer.intake(&config, 10.0, Some(0.0)), 1);
    host.snapshot(&mut peer, 1, 1.0, 10.0);
    peer.project(&config, 10.0, Some(0.0));
    approx(number(peer.facts(id), "healthFraction"), 0.7);
    approx(number(peer.facts(id), "shieldFraction"), 0.5);
    let initial_revision = peer.state.revision().unwrap();

    {
        let mut registry = host.ctx.registry.borrow_mut();
        let mut health = registry
            .get_component::<HealthComponent>(target)
            .unwrap()
            .clone();
        health.current = 95.0;
        registry.set_component(target, health).unwrap();
        let mut state = registry
            .get_component::<EntityStateComponent>(target)
            .unwrap()
            .clone();
        state.set("shield", 40.0);
        registry.set_component(target, state).unwrap();
        assert!(
            registry.take_impact_dispatches().is_empty(),
            "recharge emits no damage edge"
        );
    }
    host.frame(1.1);
    let recharged = host.tracker.roster.entries[0];
    assert!((recharged.damaged_at - original.damaged_at).abs() < 1.0e-9);
    assert!((recharged.expires_at - original.expires_at).abs() < 1.0e-9);
    approx(
        number(host.pool.overlay_facts(target).unwrap(), "healthFraction"),
        0.95,
    );
    approx(
        number(host.pool.overlay_facts(target).unwrap(), "shieldFraction"),
        0.8,
    );
    peer.deliver(host.packets(&peer));
    assert_eq!(peer.intake(&config, 10.1, Some(0.0)), 1);
    assert!(peer.state.revision().unwrap() > initial_revision);
    approx(number(peer.facts(id), "healthFraction"), 0.95);
    approx(number(peer.facts(id), "shieldFraction"), 0.8);

    // Interpolation writes the displayed Transform between snapshots. Anchor
    // projection follows that pose on every frame, keeping pushed facts intact.
    let entity = peer.replication.entity_for_network_id(id).unwrap();
    let mut displayed = *peer.registry.get_component::<Transform>(entity).unwrap();
    displayed.position = Vec3::new(3.0, 4.0, 5.0);
    peer.registry
        .set_presentation_transform(entity, displayed)
        .unwrap();
    peer.project(&config, 10.2, Some(0.0));
    let anchor = peer.pool.overlay_anchor(entity).unwrap();
    approx(anchor.x, 3.0);
    approx(anchor.y, 6.0);
    approx(anchor.z, 5.0);
    approx(number(peer.facts(id), "healthFraction"), 0.95);
    approx(number(peer.facts(id), "shieldFraction"), 0.8);
    assert!(
        peer.registry
            .get_component::<HealthComponent>(entity)
            .is_err()
    );
    let retained = peer.state.roster.as_ref().unwrap().entries[0];
    assert!((retained.expires_at - original.expires_at).abs() < 1.0e-9);
    peer.project(&config, 11.99, Some(0.0));
    assert_eq!(peer.ids(), [id]);
    peer.project(&config, 12.01, Some(0.0));
    assert!(
        peer.ids().is_empty(),
        "recharge and movement preserve original expiry"
    );
}

#[test]
fn late_join_and_repromotion_receive_only_live_remaining_lifetime() {
    let mut host = Host::new(2, 1_000);
    let mut first = Peer::connect(&mut host, 41);
    let expired = host.target(false);
    let live = host.target(false);
    host.hit(expired, None, 1.0, 1.0);
    host.frame(1.0);
    first.deliver(host.packets(&first));
    let config = host.runtime.client_overlay_config().unwrap();
    first.intake(&config, 0.0, None);
    host.server.set_mod_digest(None);
    first.deliver(host.packets(&first));
    assert!(!first.net.is_participating());
    first.state.clear();
    first.pool.clear_world_instances();
    host.hit(live, None, 2.0, 10.0);
    host.frame(2.0);
    first.deliver(host.packets(&first));
    assert!(
        first.net.drain_presentation().is_empty(),
        "held peer receives no roster"
    );
    host.server.set_mod_digest(Some([7; 32]));
    first.deliver(host.packets(&first));
    assert!(first.net.is_participating());
    let mut late = Peer::connect(&mut host, 73);
    host.frame(2.25);
    for peer in [&mut first, &mut late] {
        peer.deliver(host.packets(peer));
        assert_eq!(peer.intake(&config, 50.0, Some(0.0)), 1);
        host.snapshot(peer, 1, 2.25, 50.0);
        peer.project(&config, 50.0, Some(0.0));
        assert_eq!(peer.ids(), [host.id(live)]);
        assert!((peer.state.roster.as_ref().unwrap().entries[0].expires_at - 3.0).abs() < 1.0e-9);
        peer.project(&config, 50.76, Some(0.0));
        assert!(peer.ids().is_empty(), "join does not restart full linger");
    }
}

#[test]
fn publication_cadence_bounds_updates_and_repairs_including_short_linger() {
    for cap in [1, 8, 64] {
        for player_count in [1, 2, 4] {
            let mut host = Host::new(cap, 10_000);
            let mut peers: Vec<_> = (0..player_count)
                .map(|index| Peer::connect(&mut host, 41 + index))
                .collect();
            let target = host.target(true);
            let mut publications = vec![0usize; player_count as usize];
            for step in 0..100 {
                let now = 1.0 + step as f64 * 0.01;
                host.hit(target, None, now, 1.0);
                host.frame(now);
                for (index, peer) in peers.iter_mut().enumerate() {
                    peer.deliver(host.packets(peer));
                    publications[index] += peer.net.drain_presentation().len();
                }
            }
            assert_eq!(
                publications,
                vec![10; player_count as usize],
                "10Hz change cap, K={cap}"
            );
            publications.fill(0);
            // Last changed state publishes at 2.0. Thereafter repairs are 4Hz.
            for step in 0..101 {
                host.frame(2.0 + step as f64 * 0.01);
                for (index, peer) in peers.iter_mut().enumerate() {
                    peer.deliver(host.packets(peer));
                    publications[index] += peer.net.drain_presentation().len();
                }
            }
            assert_eq!(
                publications,
                vec![5; player_count as usize],
                "final change + four repairs, K={cap}"
            );
            eprintln!(
                "overlay cadence: K={cap}, P={player_count}, changed=10/s/client, repair=4/s/client"
            );
        }
    }
    let mut host = Host::new(1, 50);
    let mut peer = Peer::connect(&mut host, 41);
    let target = host.target(false);
    host.hit(target, None, 1.0, 1.0);
    host.frame(1.0);
    let config = host.runtime.client_overlay_config().unwrap();
    peer.deliver(host.packets(&peer));
    peer.intake(&config, 0.0, Some(0.0));
    host.snapshot(&mut peer, 1, 1.0, 0.0);
    peer.project(&config, 0.0, Some(0.0));
    assert_eq!(peer.ids(), [host.id(target)]);
    host.frame(1.05);
    peer.project(&config, 0.051, Some(0.0));
    assert!(
        peer.ids().is_empty(),
        "short linger expires before 0.1s publication cadence"
    );
    assert_eq!(host.pool.live_counts().1, 0);
    host.frame(1.1);
    peer.deliver(host.packets(&peer));
    peer.intake(&config, 0.1, Some(0.0));
    assert!(peer.state.roster.as_ref().unwrap().entries.is_empty());
}

#[test]
fn encoded_roster_cost_and_fragment_loss_repair_are_bounded_and_atomic() {
    assert_eq!(
        wire::MAX_OVERLAY_ROSTER_ENTRIES,
        postretro_scripting_core::data_descriptors::MAX_PRESENTATION_OVERLAY_VISIBLE,
        "wire capacity follows the authoring ceiling",
    );
    for cap in [1, 8, 64] {
        for varied_floats in [false, true] {
            let mut host = Host::new(cap, 10_000);
            let mut peer = Peer::connect(&mut host, 41);
            for index in 0..cap {
                let target = host.target(false);
                if varied_floats {
                    let mut registry = host.ctx.registry.borrow_mut();
                    let mut state = registry
                        .get_component::<EntityStateComponent>(target)
                        .unwrap()
                        .clone();
                    state.set("shield", 1.234_567 + index as f32 * 0.612_345);
                    registry.set_component(target, state).unwrap();
                }
                host.hit(
                    target,
                    None,
                    if varied_floats {
                        1.000_123_456 + index as f64 * 0.000_001_379
                    } else {
                        1.0
                    },
                    if varied_floats {
                        0.123_456 + index as f32 * 0.419_379
                    } else {
                        10.0
                    },
                );
            }
            host.frame(1.01);
            let message = ServerPresentationMessage {
                payload: ServerPresentationPayload::OverlayRoster(host.tracker.roster.clone()),
            };
            let encoded_bytes = wire::encode(&message).len();
            let packets = host.packets(&peer);
            let transport_bytes: usize = packets.iter().map(Vec::len).sum();
            assert!(transport_bytes >= encoded_bytes);
            assert!(
                transport_bytes < 8_192,
                "bounded complete roster fits stock tick allowance"
            );
            eprintln!(
                "overlay bytes: K={cap}, varied={varied_floats}, encoded={encoded_bytes}, framed-renet={transport_bytes}, packets={}",
                packets.len()
            );
            let config = host.runtime.client_overlay_config().unwrap();
            if cap == 64 && varied_floats {
                assert!(
                    packets.len() > 1,
                    "fixture actually reaches renet fragmentation"
                );
                // Renet exposes complete messages only. Missing a slice must
                // leave the entire roster unapplied, rather than partially replacing it.
                // Packet batches also contain small ACKs. Discard the largest
                // packet, which contains a full 1200-byte data slice, rather
                // than assuming the last packet carries roster data.
                let missing_slice = packets
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, packet)| packet.len())
                    .unwrap()
                    .0;
                assert!(
                    packets[missing_slice].len() >= 1_200,
                    "a full data slice is actually lost"
                );
                peer.deliver(
                    packets
                        .into_iter()
                        .enumerate()
                        .filter_map(|(index, packet)| (index != missing_slice).then_some(packet))
                        .collect(),
                );
                assert_eq!(peer.intake(&config, 0.0, Some(0.0)), 0);
                assert!(peer.state.roster.is_none());
                host.frame(1.26);
                peer.deliver(host.packets(&peer));
                assert_eq!(peer.intake(&config, 0.25, Some(0.0)), 1);
            } else {
                peer.deliver(packets);
                assert_eq!(peer.intake(&config, 0.0, Some(0.0)), 1);
            }
            assert_eq!(peer.state.pending_len(), cap);
            host.snapshot(&mut peer, 1, 1.26, 0.25);
            peer.project(&config, 0.25, Some(0.0));
            assert_eq!(peer.ids().len(), cap);
        }
    }
}
