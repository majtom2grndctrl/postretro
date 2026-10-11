use super::tests::{spawn_static_descriptor_prop, spawn_world_item};
use super::*;
use glam::Vec3;
use postretro_entities::FogVolumeComponent;
use postretro_entities::components::billboard_emitter::{BillboardEmitterComponent, SpinAnimation};
use postretro_entities::components::fog_volume::FogAnimation;
use postretro_entities::components::light::{
    FalloffKind, LightAnimation, LightComponent, LightKind,
};
use postretro_entities::presentation_binding::{PresentationBindingKey, PresentationBindingKind};
use postretro_foundation::Vec3Lit;

#[derive(Default)]
struct Resolver(HashMap<EntityId, PresentationBindingKey>);
impl PresentationBindingResolver for Resolver {
    fn resolve(&self, _: &EntityRegistry, key: PresentationBindingKey) -> Option<(EntityId, Vec3)> {
        self.0
            .iter()
            .find_map(|(id, value)| (*value == key).then_some((*id, Vec3::ZERO)))
    }
    fn key_for_entity(
        &self,
        _: &EntityRegistry,
        id: EntityId,
    ) -> Option<(PresentationBindingKey, Vec3)> {
        self.0.get(&id).copied().map(|key| (key, Vec3::ZERO))
    }
}
fn write(overrides: &mut PresentationOverrides, id: EntityId, component: PresentationComponent) {
    overrides.records.insert(
        (id, component.kind()),
        PresentationOverride {
            component,
            anim_start: Some(2.0),
        },
    );
}
fn produce(
    registry: &EntityRegistry,
    set: &ReplicableSet,
    allocator: &mut NetworkIdAllocator,
    overrides: &PresentationOverrides,
) -> Vec<EntitySnapshot> {
    produce_owned_snapshots_with_host_aim(
        registry,
        set,
        allocator,
        &MovementOwners::new(),
        &HostCommandQueues::new(),
        None,
        overrides,
    )
}
fn light() -> LightComponent {
    LightComponent {
        origin: [4.0, 5.0, 6.0],
        light_type: LightKind::Spot,
        intensity: 2.0,
        color: [0.2, 0.4, 0.6],
        falloff_model: FalloffKind::Linear,
        falloff_range: 12.0,
        cone_angle_inner: Some(0.2),
        cone_angle_outer: Some(0.8),
        cone_direction: Some([0.0, -1.0, 0.0]),
        is_dynamic: true,
        animated_slot: Some(7),
        follow_transform: true,
        carrier: None,
        animation: Some(LightAnimation {
            period_ms: 2000.0,
            phase: Some(0.25),
            play_count: Some(3),
            start_active: Some(true),
            brightness: Some(vec![0.25, 1.0]),
            color: Some(vec![Vec3Lit([1.0, 0.0, 0.0])]),
            direction: Some(vec![Vec3Lit([0.0, -1.0, 0.0])]),
            radius: None,
        }),
    }
}
fn fog() -> FogVolumeComponent {
    FogVolumeComponent {
        density: 0.3,
        glow: 0.2,
        edge_softness: 0.5,
        falloff: 0.7,
        tint: [0.2, 0.4, 0.6],
        saturation: 0.8,
        min_brightness: 0.1,
        light_range: 4.0,
        animation: Some(FogAnimation {
            period_ms: 5000.0,
            phase: Some(0.25),
            play_count: Some(1),
            density: Some(vec![0.3, 0.6]),
            saturation: Some(vec![1.0, 0.0]),
            min_brightness: Some(vec![0.1, 0.2]),
            light_range: Some(vec![4.0, 2.0]),
        }),
    }
}
fn emitter() -> BillboardEmitterComponent {
    BillboardEmitterComponent {
        rate: 5.0,
        burst: Some(4),
        spread: 0.3,
        lifetime: 3.0,
        velocity: [0.0, 1.0, 0.0],
        buoyancy: 0.2,
        drag: 0.1,
        size_over_lifetime: [0.5, 1.0].into(),
        opacity_over_lifetime: [1.0, 0.0].into(),
        color: [1.0, 0.5, 0.0],
        sprite: "smoke".into(),
        spin_rate: 2.0,
        spin_animation: Some(SpinAnimation {
            duration: 5.0,
            rate_curve: vec![0.0, 2.0],
        }),
    }
}

#[test]
fn presentation_pawn_host_only_capture_keeps_player_start_and_network_slot_records() {
    use crate::predict_reconcile_harness_test_fixtures::entity_descriptors;
    use postretro_entities::components::map_ordinal::MapOrdinal;
    use postretro_entities::{
        DataRegistry, FireOrigin, FireSubject, NamedReaction, PrimitiveDescriptor,
        ReactionDescriptor, ScriptCtx,
    };
    use postretro_net::replication::typed_records;
    use postretro_net::wire::{EntityRecord, SnapshotMessage};
    use postretro_scripting_core::reaction_dispatch::{
        NamedEventDispatchContext, fire_named_event_with_sequences,
    };
    use postretro_scripting_core::reaction_registry::{
        ReactionPrimitiveRegistry, SystemReactionRegistry,
    };
    use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
    use postretro_sim::scripting::builtins::{
        data_archetype::spawn_from_player_starts, data_archetype_test_fixtures::placement,
        net_descriptor::spawn_net_slot_pawn_with_carried_loadout,
    };
    use postretro_test_log_capture::LogCapture;

    for path in [
        DescriptorSpawnPath::PlayerSpawn,
        DescriptorSpawnPath::NetworkSlot,
    ] {
        // Test both the real spawn result and a stale/forged ordinal so the
        // registration guard remains covered independently of spawn attachment.
        for stale_ordinal in [false, true] {
            let ctx = ScriptCtx::new();
            ctx.presentation_override_validator
                .set(Some(crate::validate_presentation_override));
            ctx.presentation_networked_predicate
                .set(Some(is_host_replicated_descriptor_entity));
            let mut descriptors = entity_descriptors();
            descriptors[0].emitter = Some(emitter());
            let mut start = placement("player_spawn", &[]);
            // install_world_cpu stamps the full PRL roster before partitioning
            // player starts. Both authoritative pawn constructors receive it.
            start.map_ordinal = Some(17);
            start.tags = vec!["lit_player".into()];
            let pawn = {
                let mut registry = ctx.registry.borrow_mut();
                match path {
                    DescriptorSpawnPath::PlayerSpawn => {
                        assert_eq!(
                            spawn_from_player_starts(&[start], &descriptors, &mut registry, None)
                                .spawned,
                            1
                        );
                        registry.local_player_pawn().unwrap()
                    }
                    DescriptorSpawnPath::NetworkSlot => spawn_net_slot_pawn_with_carried_loadout(
                        &start,
                        &descriptors,
                        &mut registry,
                        None,
                        None,
                    )
                    .unwrap(),
                    _ => unreachable!(),
                }
            };
            assert_eq!(
                ctx.registry
                    .borrow()
                    .get_component::<DescriptorProvenance>(pawn)
                    .unwrap()
                    .spawn_path,
                path
            );
            assert!(
                !ctx.registry
                    .borrow()
                    .has_component_kind(pawn, ComponentKind::MapOrdinal)
                    .unwrap()
            );
            if stale_ordinal {
                ctx.registry
                    .borrow_mut()
                    .set_component(pawn, MapOrdinal(17))
                    .unwrap();
            }

            let mut data = DataRegistry::new();
            data.populate_level(
                vec![NamedReaction {
                    name: "pawn_smoke".into(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "setEmitterRate".into(),
                        target: None,
                        kind: None,
                        tag: Some("lit_player".into()),
                        on_complete: None,
                        args: serde_json::json!({"rate": 0.0}),
                    }),
                }],
                Vec::new(),
                &[],
            );
            let mut reactions = ReactionPrimitiveRegistry::new();
            postretro_sim::scripting::reactions::registry::register_emitter_reaction_primitives(
                &mut reactions,
            );
            let sequences = SequencedPrimitiveRegistry::new();
            let systems = SystemReactionRegistry::new();
            let resolver = Resolver(
                [(
                    pawn,
                    PresentationBindingKey {
                        kind: PresentationBindingKind::MapEntity,
                        index: 17,
                    },
                )]
                .into(),
            );
            let mut allocator = NetworkIdAllocator::new();
            let network_id = allocator.stamp(pawn);
            let mut set = ReplicableSet::new();
            set.register(pawn);
            let mut tracker = ServerReplication::new();
            tracker.register_client(1);
            tracker.register_client(2);
            let mut client_registry = EntityRegistry::new();
            let mut client = crate::ClientReplication::new();
            let logs = LogCapture::start();

            for tick in 0..3 {
                if tick > 0 {
                    ctx.presentation_overrides.borrow_mut().begin_frame();
                    ctx.presentation_script_time.set(f64::from(tick));
                    assert!(
                        fire_named_event_with_sequences(
                            "pawn_smoke",
                            &data,
                            &sequences,
                            &reactions,
                            &systems,
                            &ctx,
                            Some(NamedEventDispatchContext {
                                origin: FireOrigin::HostOnly(FireSubject::None),
                                source: "pawn_death_event".into(),
                                values: &[],
                                emitter: None,
                            }),
                        )
                        .is_empty()
                    );
                    assert_eq!(
                        ctx.registry
                            .borrow()
                            .get_component::<BillboardEmitterComponent>(pawn)
                            .unwrap()
                            .rate,
                        0.0
                    );
                    assert!(
                        ctx.presentation_overrides
                            .borrow()
                            .records
                            .contains_key(&(pawn, ComponentKind::BillboardEmitter)),
                        "successful host-only write reaches capture before being refused by registration"
                    );
                    let mut transform = *ctx
                        .registry
                        .borrow()
                        .get_component::<Transform>(pawn)
                        .unwrap();
                    transform.position.x += 1.0;
                    ctx.registry
                        .borrow_mut()
                        .set_component(pawn, transform)
                        .unwrap();
                    let mut movement = ctx
                        .registry
                        .borrow()
                        .get_component::<PlayerMovementComponent>(pawn)
                        .unwrap()
                        .clone();
                    movement.velocity.x = tick as f32;
                    ctx.registry
                        .borrow_mut()
                        .set_component(pawn, movement)
                        .unwrap();
                    if stale_ordinal {
                        set.bound_identities.insert(
                            pawn,
                            WirePresentationBinding {
                                kind: PresentationBindingKind::MapEntity as u8,
                                index: 17,
                                origin: [0.0; 3],
                            },
                        );
                    }
                }
                host_register_presentation_overrides(
                    &ctx.registry.borrow(),
                    &mut ctx.presentation_overrides.borrow_mut(),
                    &resolver,
                    &mut allocator,
                    &mut set,
                    &mut tracker,
                );
                assert!(ctx.presentation_overrides.borrow().records.is_empty());
                assert!(!set.bound_identities.contains_key(&pawn));
                assert!(set.contains(pawn));
                assert_eq!(allocator.network_id_for_entity(pawn), Some(network_id));
                let snapshots = produce(
                    &ctx.registry.borrow(),
                    &set,
                    &mut allocator,
                    &ctx.presentation_overrides.borrow(),
                );
                assert_eq!(snapshots.len(), 1);
                let snapshot = &snapshots[0];
                assert_eq!(snapshot.entity_class.as_deref(), Some("pawn:player"));
                assert!(snapshot.presentation_binding.is_none());
                assert_eq!(snapshot.components.len(), 2);
                assert!(matches!(
                    snapshot.components[0],
                    ComponentPayload::Transform(_)
                ));
                assert!(matches!(
                    snapshot.components[1],
                    ComponentPayload::PlayerMovementState(ref movement) if movement.velocity[0] == tick as f32
                ));
                tracker.ingest_tick(snapshots);
                let raw = tracker.encode_for_client(1, tick).unwrap();
                let records = typed_records(&raw);
                assert_eq!(
                    records.len(),
                    1,
                    "movement remains dirty after presentation capture"
                );
                if tick == 0 {
                    assert!(matches!(records[0], EntityRecord::FullBaseline { .. }));
                } else {
                    assert!(matches!(records[0], EntityRecord::Delta { .. }));
                }
                let outcome = client.apply_snapshot(
                    &mut client_registry,
                    &SnapshotMessage {
                        sequence: raw.sequence,
                        server_tick: tick,
                        records,
                        state_schema_fingerprint: [0; 32],
                        state_records: Vec::new(),
                        faction_sentiment_record: None,
                        server_script_time: f64::from(tick),
                    },
                );
                let remote = client
                    .entity_for_network_id(network_id)
                    .expect("pawn applies without a client-local player-start binding");
                assert_eq!(
                    client_registry
                        .get_component::<Transform>(remote)
                        .unwrap()
                        .position,
                    ctx.registry
                        .borrow()
                        .get_component::<Transform>(pawn)
                        .unwrap()
                        .position
                );
                let ack = outcome.ack.expect("pawn snapshot acknowledges normally");
                tracker.apply_ack(
                    1,
                    raw.sequence,
                    &ack.entity_baselines,
                    &ack.despawn_tombstones,
                );
            }
            logs.assert_logged_once(
                log::Level::Warn,
                &format!("cannot bind presentation override for entity {pawn:?}"),
            );
            let join = tracker.encode_for_client(2, 3).unwrap();
            assert!(
                matches!(&typed_records(&join)[0], EntityRecord::FullBaseline {
                entity_class: Some(class), presentation_binding: None, components, ..
            } if class == "pawn:player" && components.iter().any(|component| matches!(component, ComponentPayload::PlayerMovementState(_))))
            );
            let mut late_client = crate::ClientReplication::new();
            let mut late_registry = EntityRegistry::new();
            let late_outcome = late_client.apply_snapshot(
                &mut late_registry,
                &SnapshotMessage {
                    sequence: join.sequence,
                    server_tick: 3,
                    records: typed_records(&join),
                    state_schema_fingerprint: [0; 32],
                    state_records: Vec::new(),
                    faction_sentiment_record: None,
                    server_script_time: 3.0,
                },
            );
            assert!(late_outcome.refresh_requests.is_empty());
            assert_eq!(late_outcome.remote_entities[0].entity_class, "pawn:player");
            let late_pawn = late_client
                .entity_for_network_id(network_id)
                .expect("late join materializes the pawn without any local player-start copy");
            assert_eq!(
                late_registry
                    .get_component::<Transform>(late_pawn)
                    .unwrap()
                    .position,
                ctx.registry
                    .borrow()
                    .get_component::<Transform>(pawn)
                    .unwrap()
                    .position
            );
        }
    }
}

#[test]
fn presentation_registration_keeps_projectile_path_and_binds_genuine_movement_placement() {
    use postretro_entities::components::map_ordinal::MapOrdinal;
    use postretro_sim::scripting::builtins::{
        data_archetype::apply_data_archetype_dispatch, data_archetype_test_fixtures::placement,
    };
    use postretro_test_log_capture::LogCapture;

    let mut registry = EntityRegistry::new();
    let mut descriptors = crate::predict_reconcile_harness_test_fixtures::entity_descriptors();
    descriptors[0].emitter = Some(emitter());
    let mut placed = placement("player", &[]);
    placed.map_ordinal = Some(0);
    apply_data_archetype_dispatch(
        &[placed],
        &descriptors,
        &HashSet::new(),
        &mut registry,
        None,
    );
    let map_id = registry
        .iter_with_kind(ComponentKind::DescriptorProvenance)
        .next()
        .unwrap()
        .0;
    assert!(
        registry
            .has_component_kind(map_id, ComponentKind::PlayerMovement)
            .unwrap()
    );
    assert!(!is_host_replicated_descriptor_entity(&registry, map_id));
    assert_eq!(registry.get_component::<MapOrdinal>(map_id).unwrap().0, 0);

    let projectile = registry.spawn(Transform::default());
    registry.set_component(projectile, MapOrdinal(1)).unwrap();
    registry
        .set_component(
            projectile,
            DescriptorProvenance {
                canonical_name: "bolt".into(),
                spawn_path: DescriptorSpawnPath::ProjectilePresentation,
                owned_components: Default::default(),
                map_overrides: Default::default(),
            },
        )
        .unwrap();
    let resolver = Resolver(
        [
            (
                map_id,
                PresentationBindingKey {
                    kind: PresentationBindingKind::MapEntity,
                    index: 0,
                },
            ),
            (
                projectile,
                PresentationBindingKey {
                    kind: PresentationBindingKind::MapEntity,
                    index: 1,
                },
            ),
        ]
        .into(),
    );
    let mut allocator = NetworkIdAllocator::new();
    let projectile_network_id = allocator.stamp(projectile);
    let mut set = ReplicableSet::new();
    let facts = postretro_net::wire::WireProjectilePresentation {
        sprite_size: Some(0.5),
        model_scale: 1.0,
        shot_id: None,
    };
    set.register_projectile_presentation(projectile, facts);
    let mut tracker = ServerReplication::new();
    let mut overrides = PresentationOverrides::default();
    let logs = LogCapture::start();
    for _ in 0..2 {
        write(
            &mut overrides,
            projectile,
            PresentationComponent::Emitter(emitter()),
        );
        write(
            &mut overrides,
            map_id,
            PresentationComponent::Emitter(emitter()),
        );
        host_register_presentation_overrides(
            &registry,
            &mut overrides,
            &resolver,
            &mut allocator,
            &mut set,
            &mut tracker,
        );
        assert!(!overrides.records.keys().any(|(id, _)| *id == projectile));
        assert_eq!(
            allocator.network_id_for_entity(projectile),
            Some(projectile_network_id)
        );
        let snapshots = produce(&registry, &set, &mut allocator, &overrides);
        let projectile_snapshot = snapshots
            .iter()
            .find(|snapshot| snapshot.network_id == projectile_network_id.0)
            .unwrap();
        assert_eq!(
            projectile_snapshot.entity_class.as_deref(),
            Some("projectile:bolt")
        );
        assert_eq!(projectile_snapshot.projectile_presentation, Some(facts));
        assert!(projectile_snapshot.presentation_binding.is_none());
        assert!(matches!(
            projectile_snapshot.components.as_slice(),
            [ComponentPayload::Transform(_)]
        ));
        let placement_snapshot = snapshots
            .iter()
            .find(|snapshot| snapshot.network_id != projectile_network_id.0)
            .unwrap();
        assert_eq!(placement_snapshot.presentation_binding.unwrap().index, 0);
        assert!(matches!(
            placement_snapshot.components.as_slice(),
            [ComponentPayload::EmitterState(_)]
        ));
    }
    logs.assert_logged_once(
        log::Level::Warn,
        &format!("cannot bind presentation override for entity {projectile:?}"),
    );
}

#[test]
fn presentation_registration_sorted_multi_kind_immutable_and_last_valid() {
    let mut registry = EntityRegistry::new();
    let a = registry.spawn(Transform::default());
    let b = registry.spawn(Transform::default());
    registry.set_component(a, fog()).unwrap();
    let mut overrides = PresentationOverrides::default();
    write(&mut overrides, b, PresentationComponent::Light(light()));
    write(&mut overrides, a, PresentationComponent::Fog(fog()));
    write(&mut overrides, a, PresentationComponent::Emitter(emitter()));
    let resolver = Resolver(
        [
            (
                a,
                PresentationBindingKey {
                    kind: PresentationBindingKind::FogVolume,
                    index: 0,
                },
            ),
            (
                b,
                PresentationBindingKey {
                    kind: PresentationBindingKind::MapLight,
                    index: 1,
                },
            ),
        ]
        .into(),
    );
    let mut allocator = NetworkIdAllocator::new();
    let mut set = ReplicableSet::new();
    let mut tracker = ServerReplication::new();
    tracker.register_client(1);
    host_register_presentation_overrides(
        &registry,
        &mut overrides,
        &resolver,
        &mut allocator,
        &mut set,
        &mut tracker,
    );
    assert!(
        allocator.network_id_for_entity(a).unwrap().0
            < allocator.network_id_for_entity(b).unwrap().0
    );
    let mut sampled_fog = registry
        .get_component::<FogVolumeComponent>(a)
        .unwrap()
        .clone();
    sampled_fog.density = 0.99;
    registry.set_component(a, sampled_fog).unwrap();
    let first = produce(&registry, &set, &mut allocator, &overrides);
    let a_snapshot = first
        .iter()
        .find(|s| s.network_id == allocator.network_id_for_entity(a).unwrap().0)
        .unwrap();
    assert_eq!(a_snapshot.components.len(), 2);
    assert!(a_snapshot.presentation_binding.is_some());
    assert!(a_snapshot.entity_class.is_none());
    assert!(
        matches!(&a_snapshot.components[0], ComponentPayload::FogState(fog) if fog.density == 0.3)
    );
    assert!(
        first
            .iter()
            .flat_map(|s| &s.components)
            .all(|p| !matches!(p, ComponentPayload::Transform(_)))
    );
    tracker.ingest_tick(first.clone());
    let wire = tracker.encode_for_client(1, 0).unwrap();
    let baselines: Vec<_> = first
        .iter()
        .map(|snapshot| {
            (
                snapshot.network_id,
                tracker.current_baseline_id(snapshot.network_id).unwrap(),
            )
        })
        .collect();
    tracker.apply_ack(1, wire.sequence, &baselines, &[]);
    tracker.ingest_tick(produce(&registry, &set, &mut allocator, &overrides));
    assert!(
        tracker.encode_for_client(1, 1).unwrap().records.is_empty(),
        "unchanged immutable records do not dirty their baseline"
    );
    let bad = overrides
        .records
        .get_mut(&(a, ComponentKind::FogVolume))
        .unwrap();
    if let PresentationComponent::Fog(fog) = &mut bad.component {
        fog.density = f32::NAN;
    }
    let fallback = produce(&registry, &set, &mut allocator, &overrides);
    assert_eq!(fallback, first);
    tracker.ingest_tick(fallback);
    assert!(
        tracker
            .current_baseline_id(allocator.network_id_for_entity(a).unwrap().0)
            .is_some()
    );
    let net = allocator.network_id_for_entity(a).unwrap().0;
    registry.despawn(a).unwrap();
    host_register_presentation_overrides(
        &registry,
        &mut overrides,
        &resolver,
        &mut allocator,
        &mut set,
        &mut tracker,
    );
    tracker.ingest_tick(produce(&registry, &set, &mut allocator, &overrides));
    assert!(
        tracker
            .encode_for_client(1, 1)
            .unwrap()
            .records
            .iter()
            .all(|record| record.network_id != net)
    );
    assert!(!set.contains(a));
    assert!(!overrides.records.keys().any(|(id, _)| *id == a));
    assert!(
        !set.last_valid_payloads
            .borrow()
            .keys()
            .any(|(id, _)| *id == a)
    );
}

#[test]
fn presentation_registration_first_invalid_and_unresolvable_stay_off_wire() {
    let mut registry = EntityRegistry::new();
    let invalid = registry.spawn(Transform::default());
    let unknown = registry.spawn(Transform::default());
    let mut overrides = PresentationOverrides::default();
    let mut bad = light();
    bad.intensity = f32::NAN;
    write(&mut overrides, invalid, PresentationComponent::Light(bad));
    write(
        &mut overrides,
        unknown,
        PresentationComponent::Light(light()),
    );
    let resolver = Resolver(
        [(
            invalid,
            PresentationBindingKey {
                kind: PresentationBindingKind::MapLight,
                index: 0,
            },
        )]
        .into(),
    );
    let mut allocator = NetworkIdAllocator::new();
    let mut set = ReplicableSet::new();
    let mut tracker = ServerReplication::new();
    host_register_presentation_overrides(
        &registry,
        &mut overrides,
        &resolver,
        &mut allocator,
        &mut set,
        &mut tracker,
    );
    assert!(produce(&registry, &set, &mut allocator, &overrides).is_empty());
    assert!(
        !overrides
            .records
            .contains_key(&(unknown, ComponentKind::Light))
    );
    assert_eq!(overrides.warned_unresolvable.len(), 1);
    write(
        &mut overrides,
        unknown,
        PresentationComponent::Light(light()),
    );
    host_register_presentation_overrides(
        &registry,
        &mut overrides,
        &resolver,
        &mut allocator,
        &mut set,
        &mut tracker,
    );
    assert_eq!(overrides.warned_unresolvable.len(), 1);
}

#[test]
fn presentation_descriptor_kvps_and_held_overrides_survive_drop() {
    let mut registry = EntityRegistry::new();
    let item = spawn_world_item(&mut registry, "item");
    registry.set_component(item, light()).unwrap();
    registry.set_component(item, emitter()).unwrap();
    let mut provenance = registry
        .get_component::<DescriptorProvenance>(item)
        .unwrap()
        .clone();
    provenance.map_overrides.extend([
        DescriptorMapOverride::LightInitialColor,
        DescriptorMapOverride::EmitterInitialRate,
    ]);
    registry.set_component(item, provenance).unwrap();
    let mut allocator = NetworkIdAllocator::new();
    let mut set = ReplicableSet::new();
    let mut tracked = HashSet::new();
    let mut overrides = PresentationOverrides::default();
    host_register_descriptor_entities(
        &registry,
        &mut allocator,
        &mut set,
        &mut tracked,
        &mut overrides,
    );
    assert_eq!(overrides.records.len(), 2);
    assert!(
        overrides
            .records
            .values()
            .all(|record| record.anim_start.is_none())
    );
    let initial = allocator.network_id_for_entity(item).unwrap();
    let holder = registry.spawn(Transform::default());
    let mut inventory = postretro_entities::components::inventory::Inventory::default();
    inventory.wieldables[0] = Some(item);
    registry.set_component(holder, inventory).unwrap();
    host_register_descriptor_entities(
        &registry,
        &mut allocator,
        &mut set,
        &mut tracked,
        &mut overrides,
    );
    write(
        &mut overrides,
        item,
        PresentationComponent::Light({
            let mut value = light();
            value.intensity = 9.0;
            value
        }),
    );
    let mut tracker = ServerReplication::new();
    host_register_presentation_overrides(
        &registry,
        &mut overrides,
        &Resolver::default(),
        &mut allocator,
        &mut set,
        &mut tracker,
    );
    assert_eq!(overrides.records.len(), 2);
    assert!(!set.contains(item));
    assert!(overrides.warned_unresolvable.is_empty());
    registry
        .set_component(
            holder,
            postretro_entities::components::inventory::Inventory::default(),
        )
        .unwrap();
    host_register_descriptor_entities(
        &registry,
        &mut allocator,
        &mut set,
        &mut tracked,
        &mut overrides,
    );
    let snapshot = produce(&registry, &set, &mut allocator, &overrides)
        .pop()
        .unwrap();
    assert!(snapshot.network_id > initial.0);
    assert!(snapshot.presentation_binding.is_none());
    assert!(
        matches!(&snapshot.components[1], ComponentPayload::LightState(light) if light.intensity == 9.0)
    );
    assert_eq!(snapshot.components.len(), 3);
}

#[test]
fn presentation_descriptor_no_override_and_invalid_placement_capture() {
    let mut registry = EntityRegistry::new();
    let plain = spawn_static_descriptor_prop(&mut registry, "plain");
    registry.set_component(plain, light()).unwrap();
    let invalid = spawn_static_descriptor_prop(&mut registry, "invalid");
    let mut bad = light();
    bad.animation.as_mut().unwrap().brightness =
        Some(vec![1.0; postretro_net::wire::MAX_WIRE_CURVE_KEYS + 1]);
    registry.set_component(invalid, bad).unwrap();
    let mut provenance = registry
        .get_component::<DescriptorProvenance>(invalid)
        .unwrap()
        .clone();
    provenance
        .map_overrides
        .insert(DescriptorMapOverride::LightInitialIntensity);
    registry.set_component(invalid, provenance).unwrap();
    let mut allocator = NetworkIdAllocator::new();
    let mut set = ReplicableSet::new();
    let mut tracked = HashSet::new();
    let mut overrides = PresentationOverrides::default();
    for _ in 0..2 {
        host_register_descriptor_entities(
            &registry,
            &mut allocator,
            &mut set,
            &mut tracked,
            &mut overrides,
        );
    }
    assert!(overrides.records.is_empty());
    assert_eq!(overrides.warned_writes.len(), 1);
    let snapshots = produce(&registry, &set, &mut allocator, &overrides);
    assert_eq!(snapshots.len(), 2);
    assert!(
        snapshots
            .iter()
            .flat_map(|s| &s.components)
            .all(|p| !matches!(
                p,
                ComponentPayload::LightState(_)
                    | ComponentPayload::EmitterState(_)
                    | ComponentPayload::FogState(_)
            ))
    );
}
