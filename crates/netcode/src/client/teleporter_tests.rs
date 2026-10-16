// Client teleporter binding and observer-starvation behavior.
use super::*;
use postretro_entities::TeleporterComponent;
use postretro_net::wire::{
    WireGroundRef, WireMovementState, WirePlayerMovementState, WireTeleportState,
    WireTeleporterState, WireTransform,
};

fn transform(x: f32) -> ComponentPayload {
    ComponentPayload::Transform(WireTransform {
        position: [x, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    })
}

fn movement(generation: u8, last_teleporter: u16, ground: WireGroundRef) -> ComponentPayload {
    ComponentPayload::PlayerMovementState(WirePlayerMovementState {
        teleport: WireTeleportState::new(generation, 0.0, last_teleporter, &[]).unwrap(),
        knockback_velocity: [0.0; 3],
        push_velocity: [0.0; 3],
        velocity: [2.0, 0.0, 0.0],
        ground,
        air_jumps_remaining: 0,
        air_dashes_remaining: 0,
        dash_cooldown_ms: 0.0,
        air_ticks: 0,
        movement_state: WireMovementState::Normal,
        coyote_timer_ms: 0.0,
        jump_buffer_timer_ms: 0.0,
        jump_spent: false,
        swim_reentry_latched: false,
        capsule_half_height: 0.8,
        capsule_eye_height: 1.5,
        aim_pitch: 0.0,
    })
}

fn teleporter(source_entity: u32) -> ComponentPayload {
    ComponentPayload::TeleporterState(WireTeleporterState {
        index: 0,
        source_entity,
        armed: false,
    })
}

fn baseline(network_id: u32, baseline_id: u32, components: Vec<ComponentPayload>) -> EntityRecord {
    EntityRecord::FullBaseline {
        network_id,
        baseline_id,
        last_processed_client_tick: None,
        local_player: false,
        entity_class: None,
        active_weapon_archetype: None,
        projectile_presentation: None,
        presentation_binding: None,
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
        faction_sentiment_record: None,
        server_script_time: 0.0,
    }
}

fn installed_teleporter(registry: &mut EntityRegistry) -> EntityId {
    let installed = registry.spawn(Transform::default());
    registry
        .set_component(
            installed,
            TeleporterComponent {
                index: 0,
                source_entity: 10,
                armed: true,
                arrival_sound: String::new(),
                on_teleport: String::new(),
            },
        )
        .unwrap();
    installed
}

fn armed(registry: &EntityRegistry, installed: EntityId) -> bool {
    registry
        .get_component::<TeleporterComponent>(installed)
        .unwrap()
        .armed
}

#[test]
fn teleport_baseline_naming_no_installed_teleporter_is_ignored_and_unbound() {
    let mut registry = EntityRegistry::new();
    let installed = installed_teleporter(&mut registry);
    let mut client = ClientReplication::new();
    let entities_before = registry.iter_with_kind(ComponentKind::Transform).count();
    let outcome = client.apply_snapshot(
        &mut registry,
        &snapshot(1, 1, vec![baseline(7, 1, vec![teleporter(11)])]),
    );
    assert_eq!(
        outcome.ignored,
        vec![IgnoredPayload::UnknownTeleporter {
            network_id: 7,
            index: 0,
            source_entity: 11,
        }]
    );
    assert!(client.entity_for_network_id(NetworkId(7)).is_none());
    assert_eq!(
        registry.iter_with_kind(ComponentKind::Transform).count(),
        entities_before,
        "nothing spawned"
    );
    assert!(
        armed(&registry, installed),
        "the installed region keeps its state"
    );
}

#[test]
fn teleport_baseline_that_fails_to_apply_rolls_its_binding_back() {
    let mut registry = EntityRegistry::new();
    let installed = installed_teleporter(&mut registry);
    let mut client = ClientReplication::new();
    // A payload naming an unknown ground mover fails the whole record after the
    // binding was made: nothing of it may survive.
    let outcome = client.apply_snapshot(
        &mut registry,
        &snapshot(
            1,
            1,
            vec![baseline(
                7,
                1,
                vec![
                    teleporter(10),
                    movement(0, u16::MAX, WireGroundRef::Mover(99)),
                ],
            )],
        ),
    );
    assert!(matches!(
        outcome.ignored.as_slice(),
        [IgnoredPayload::UnknownGroundMover { mover_id: 99, .. }]
    ));
    assert!(client.entity_for_network_id(NetworkId(7)).is_none());
    assert!(
        !client
            .bindings
            .teleporter_network_ids
            .contains_key(&NetworkId(7))
    );
    assert!(armed(&registry, installed));
    // The region is free: a well-formed baseline binds it.
    client.apply_snapshot(
        &mut registry,
        &snapshot(2, 2, vec![baseline(8, 1, vec![teleporter(10)])]),
    );
    assert_eq!(client.entity_for_network_id(NetworkId(8)), Some(installed));
    assert!(!armed(&registry, installed));
}

#[test]
fn teleport_starved_arrival_cut_feeds_adaptive_delay_like_a_held_pose() {
    let mut registry = EntityRegistry::new();
    let mut client = ClientReplication::new();
    client.apply_snapshot(
        &mut registry,
        &snapshot(
            1,
            10,
            vec![baseline(
                7,
                1,
                vec![transform(0.0), movement(0, u16::MAX, WireGroundRef::World)],
            )],
        ),
    );
    client.apply_snapshot(
        &mut registry,
        &snapshot(
            2,
            11,
            vec![baseline(
                7,
                2,
                vec![transform(100.0), movement(1, 0, WireGroundRef::World)],
            )],
        ),
    );
    // Bracketed by the cut: not starved.
    let stats = client.sample_into_registry(&mut registry, 10.5, 0.0);
    assert_eq!((stats.held_newest, stats.starvation_feedback), (0, 0));
    // Past the newest sample, which arrived through a teleport: the cut holds
    // it, and a still-moving pawn feeds the adaptive delay as starvation.
    let stats = client.sample_into_registry(&mut registry, 15.0, 0.0);
    assert_eq!((stats.held_newest, stats.starvation_feedback), (1, 1));
}
