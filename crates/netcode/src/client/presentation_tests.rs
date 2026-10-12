use super::*;
use postretro_entities::components::light::{FalloffKind, LightComponent, LightKind};
use postretro_entities::presentation_binding::{PresentationBindingKey, PresentationBindingKind};
use postretro_net::wire::{
    WireLightAnimation, WireLightState, WirePresentationBinding, WireTransform,
};

#[derive(Default)]
struct Resolver {
    entity: Option<EntityId>,
    seeds: Vec<(EntityId, Option<f64>)>,
}
impl PresentationBindingResolver for Resolver {
    fn resolve(&self, _: &EntityRegistry, key: PresentationBindingKey) -> Option<(EntityId, Vec3)> {
        (key == key_for_light()).then_some((self.entity?, Vec3::ZERO))
    }
    fn key_for_entity(
        &self,
        _: &EntityRegistry,
        entity: EntityId,
    ) -> Option<(PresentationBindingKey, Vec3)> {
        (self.entity == Some(entity)).then_some((key_for_light(), Vec3::ZERO))
    }
    fn seed_light(&mut self, _: &EntityRegistry, entity: EntityId, start: Option<f64>) -> bool {
        self.seeds.push((entity, start));
        true
    }
}
fn key_for_light() -> PresentationBindingKey {
    PresentationBindingKey {
        kind: PresentationBindingKind::MapLight,
        index: 0,
    }
}
fn light() -> LightComponent {
    LightComponent {
        origin: [0.0; 3],
        light_type: LightKind::Point,
        intensity: 1.0,
        color: [1.0; 3],
        falloff_model: FalloffKind::Linear,
        falloff_range: 5.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        is_dynamic: true,
        animated_slot: None,
        follow_transform: false,
        carrier: None,
        animation: None,
    }
}
fn spawn_light(registry: &mut EntityRegistry) -> EntityId {
    let entity = registry.spawn(Transform::default());
    registry.set_component(entity, light()).unwrap();
    entity
}
fn state(intensity: f32) -> ComponentPayload {
    ComponentPayload::LightState(WireLightState {
        intensity,
        color: [1.0; 3],
        cone_direction: None,
        animation: Some(WireLightAnimation {
            period_ms: 10000.0,
            phase: None,
            play_count: Some(1),
            start_active: Some(true),
            brightness: Some(vec![0.0, 1.0]),
            color: None,
            direction: None,
        }),
        anim_start: Some(5.0),
    })
}
fn binding() -> WirePresentationBinding {
    WirePresentationBinding {
        kind: 0,
        index: 0,
        origin: [0.0; 3],
    }
}
fn full(nid: u32, baseline: u32, bound: bool, payloads: Vec<ComponentPayload>) -> EntityRecord {
    EntityRecord::FullBaseline {
        network_id: nid,
        baseline_id: baseline,
        components: payloads,
        local_player: false,
        last_processed_client_tick: None,
        entity_class: (!bound).then(|| "descriptor:lamp".into()),
        active_weapon_archetype: None,
        projectile_presentation: None,
        presentation_binding: bound.then(binding),
    }
}
fn delta(
    nid: u32,
    baseline: u32,
    next: u32,
    bound: bool,
    payloads: Vec<ComponentPayload>,
) -> EntityRecord {
    EntityRecord::Delta {
        network_id: nid,
        baseline_ref: baseline,
        new_baseline_id: next,
        components: payloads,
        local_player: false,
        last_processed_client_tick: None,
        entity_class: (!bound).then(|| "descriptor:lamp".into()),
        active_weapon_archetype: None,
        projectile_presentation: None,
        presentation_binding: bound.then(binding),
    }
}
fn tombstone(nid: u32) -> EntityRecord {
    EntityRecord::Despawn {
        network_id: nid,
        tombstone_id: 9,
        reason: 0,
    }
}
fn snapshot(sequence: u32, clock: f64, records: Vec<EntityRecord>) -> SnapshotMessage {
    SnapshotMessage {
        sequence,
        server_tick: sequence,
        records,
        state_schema_fingerprint: [0; 32],
        state_records: Vec::new(),
        faction_sentiment_record: None,
        server_script_time: clock,
    }
}
fn apply(
    client: &mut ClientReplication,
    registry: &mut EntityRegistry,
    resolver: &mut Resolver,
    sequence: u32,
    clock: f64,
    records: Vec<EntityRecord>,
) -> ApplyOutcome {
    client.apply_snapshot_with_presentation(
        registry,
        &snapshot(sequence, clock, records),
        sequence,
        1.0 / 60.0,
        resolver,
        100.0,
    )
}
fn batch(
    client: &mut ClientReplication,
    registry: &mut EntityRegistry,
    resolver: &mut Resolver,
    payloads: Vec<(NetworkId, ComponentPayload)>,
    local: f64,
    rtt: Option<f64>,
) -> usize {
    client.apply_presentation_batch(registry, payloads, local, rtt, resolver)
}
fn intensity(registry: &EntityRegistry, id: EntityId) -> f32 {
    registry
        .get_component::<LightComponent>(id)
        .unwrap()
        .intensity
}

#[test]
fn rejected_binding_acks_without_repair_until_demotion_rechecks() {
    let mut registry = EntityRegistry::new();
    let authored = spawn_light(&mut registry);
    let mut client = ClientReplication::new();
    let mut resolver = Resolver::default();
    let rejected = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        1,
        10.0,
        vec![full(7, 1, true, vec![state(2.0)])],
    );
    assert_eq!(rejected.ack.unwrap().entity_baselines, vec![(7, 1)]);
    assert!(rejected.refresh_requests.is_empty());
    assert!(rejected.presentation_payloads.is_empty());
    resolver.entity = Some(authored);
    let repeat = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        2,
        11.0,
        vec![
            full(7, 2, true, vec![state(3.0)]),
            delta(7, 99, 3, true, vec![state(4.0)]),
        ],
    );
    assert_eq!(repeat.ack.unwrap().entity_baselines, vec![(7, 2), (7, 3)]);
    assert!(repeat.refresh_requests.is_empty());
    assert!(client.map().is_empty());
    assert!(client.tick_pending_repairs(1000.0).is_empty());
    client.despawn_all_mapped(&mut registry);
    client.reset_for_demotion();
    let accepted = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        3,
        12.0,
        vec![full(7, 4, true, vec![state(4.0)])],
    );
    assert_eq!(client.entity_for_network_id(NetworkId(7)), Some(authored));
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            accepted.presentation_payloads,
            100.0,
            Some(0.0)
        ),
        1
    );
    assert_eq!(intensity(&registry, authored), 4.0);
}

#[test]
fn bound_demotion_and_defensive_tombstone_keep_authored_entity_and_clear_memo() {
    let mut registry = EntityRegistry::new();
    let authored = spawn_light(&mut registry);
    let mut client = ClientReplication::new();
    let mut resolver = Resolver {
        entity: Some(authored),
        ..Default::default()
    };
    let first = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        1,
        10.0,
        vec![full(7, 1, true, vec![state(2.0)])],
    );
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            first.presentation_payloads,
            100.0,
            Some(0.0)
        ),
        1
    );
    client.despawn_all_mapped(&mut registry);
    client.reset_for_demotion();
    assert!(registry.exists(authored));
    assert!(client.map().is_empty());
    registry.set_component(authored, light()).unwrap();
    let rebound = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        2,
        11.0,
        vec![full(7, 1, true, vec![state(2.0)])],
    );
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            rebound.presentation_payloads,
            100.0,
            Some(0.0)
        ),
        1
    );
    let retired = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        3,
        12.0,
        vec![tombstone(7)],
    );
    assert_eq!(retired.ack.unwrap().despawn_tombstones, vec![(7, 9)]);
    assert!(registry.exists(authored));
    assert!(client.map().is_empty());
    let rebound = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        4,
        13.0,
        vec![full(8, 2, true, vec![state(2.0)])],
    );
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            rebound.presentation_payloads,
            100.0,
            Some(0.0)
        ),
        1
    );
    assert_eq!(resolver.seeds.len(), 3);
}

#[test]
fn accepted_batch_uses_highest_clock_and_ignores_stale_duplicate_headers() {
    let mut registry = EntityRegistry::new();
    let authored = spawn_light(&mut registry);
    let mut client = ClientReplication::new();
    let mut resolver = Resolver {
        entity: Some(authored),
        ..Default::default()
    };
    let first = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        5,
        10.0,
        vec![full(7, 1, true, vec![state(2.0)])],
    );
    let latest = apply(&mut client, &mut registry, &mut resolver, 6, 20.0, vec![]);
    assert!(latest.ack.is_some());
    for sequence in [5, 6] {
        let stale = apply(
            &mut client,
            &mut registry,
            &mut resolver,
            sequence,
            999.0,
            vec![tombstone(7)],
        );
        assert!(stale.ack.is_none());
        assert!(stale.presentation_payloads.is_empty());
    }
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            first.presentation_payloads,
            102.0,
            Some(200000.0)
        ),
        1
    );
    // Host now = 20 + 0.1 RTT + 2 local elapsed; animation age = 17.1.
    assert!((resolver.seeds[0].1.unwrap() - 84.9).abs() < 1e-8);
    assert!(registry.exists(authored));
    let mut collected = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        7,
        21.0,
        vec![delta(7, 1, 2, true, vec![state(8.0)])],
    )
    .presentation_payloads;
    collected.extend(
        apply(
            &mut client,
            &mut registry,
            &mut resolver,
            8,
            22.0,
            vec![tombstone(7)],
        )
        .presentation_payloads,
    );
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            collected,
            102.0,
            Some(0.0)
        ),
        0
    );
    assert_eq!(intensity(&registry, authored), 2.0);
}

#[test]
fn unbound_state_waits_for_materialization_then_delta_updates_complete_copy() {
    let mut registry = EntityRegistry::new();
    let mut client = ClientReplication::new();
    let mut resolver = Resolver::default();
    let transform = ComponentPayload::Transform(WireTransform {
        position: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    });
    let first = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        1,
        10.0,
        vec![full(7, 1, false, vec![transform, state(3.0)])],
    );
    assert_eq!(first.remote_entities.len(), 1);
    let copy = client.entity_for_network_id(NetworkId(7)).unwrap();
    assert!(registry.get_component::<LightComponent>(copy).is_err());
    // The descriptor materializer installs authored components before frame batch apply.
    registry.set_component(copy, light()).unwrap();
    client.cache_remote_presentation_complete(NetworkId(7), true);
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            first.presentation_payloads,
            100.0,
            Some(0.0)
        ),
        1
    );
    assert_eq!(intensity(&registry, copy), 3.0);
    let changed = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        2,
        11.0,
        vec![delta(7, 1, 2, false, vec![state(8.0)])],
    );
    assert!(changed.remote_entities.is_empty());
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            changed.presentation_payloads,
            100.0,
            Some(0.0)
        ),
        1
    );
    assert_eq!(client.entity_for_network_id(NetworkId(7)), Some(copy));
    assert_eq!(intensity(&registry, copy), 8.0);
}

#[test]
fn mapping_displacement_reapplies_identical_payload_and_drops_old_deferred_work() {
    let mut registry = EntityRegistry::new();
    let a = spawn_light(&mut registry);
    let b = spawn_light(&mut registry);
    let mut client = ClientReplication::new();
    let mut resolver = Resolver {
        entity: Some(a),
        ..Default::default()
    };
    let first = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        1,
        10.0,
        vec![full(7, 1, true, vec![state(2.0)])],
    );
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            first.presentation_payloads,
            100.0,
            Some(0.0)
        ),
        1
    );
    resolver.entity = Some(b);
    let replacement = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        2,
        11.0,
        vec![full(7, 2, true, vec![state(2.0)])],
    );
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            replacement.presentation_payloads,
            100.0,
            Some(0.0)
        ),
        1
    );
    assert_eq!(intensity(&registry, a), 2.0);
    assert_eq!(intensity(&registry, b), 2.0);
    let pending = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        3,
        12.0,
        vec![delta(7, 2, 3, true, vec![state(9.0)])],
    );
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            pending.presentation_payloads,
            100.0,
            None
        ),
        0
    );
    let displaced = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        4,
        13.0,
        vec![full(8, 4, true, vec![state(2.0)])],
    );
    assert!(client.entity_for_network_id(NetworkId(7)).is_none());
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            displaced.presentation_payloads,
            100.0,
            Some(0.0)
        ),
        1
    );
    assert_eq!(intensity(&registry, b), 2.0);
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            vec![],
            101.0,
            Some(0.0)
        ),
        0
    );
}

#[test]
fn deferred_clock_release_newer_replacement_and_dropped_mapping() {
    let mut registry = EntityRegistry::new();
    let authored = spawn_light(&mut registry);
    let mut client = ClientReplication::new();
    let mut resolver = Resolver {
        entity: Some(authored),
        ..Default::default()
    };
    let first = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        1,
        10.0,
        vec![full(7, 1, true, vec![state(2.0)])],
    );
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            first.presentation_payloads,
            100.0,
            None
        ),
        0
    );
    assert_eq!(intensity(&registry, authored), 1.0);
    assert!(resolver.seeds.is_empty());
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            vec![],
            102.0,
            Some(0.0)
        ),
        1
    );
    assert_eq!(resolver.seeds[0].1, Some(95.0));
    let older = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        2,
        11.0,
        vec![delta(7, 1, 2, true, vec![state(3.0)])],
    );
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            older.presentation_payloads,
            100.0,
            None
        ),
        0
    );
    let newer = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        3,
        12.0,
        vec![delta(7, 2, 3, true, vec![state(4.0)])],
    );
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            newer.presentation_payloads,
            100.0,
            Some(0.0)
        ),
        1
    );
    assert_eq!(intensity(&registry, authored), 4.0);
    assert_eq!(resolver.seeds.len(), 2);
    let dropped = apply(
        &mut client,
        &mut registry,
        &mut resolver,
        4,
        13.0,
        vec![delta(7, 3, 4, true, vec![state(9.0)])],
    );
    batch(
        &mut client,
        &mut registry,
        &mut resolver,
        dropped.presentation_payloads,
        100.0,
        None,
    );
    apply(
        &mut client,
        &mut registry,
        &mut resolver,
        5,
        14.0,
        vec![tombstone(7)],
    );
    assert_eq!(
        batch(
            &mut client,
            &mut registry,
            &mut resolver,
            vec![],
            101.0,
            Some(0.0)
        ),
        0
    );
    assert_eq!(intensity(&registry, authored), 4.0);
}
