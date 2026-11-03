// Remote assist retention: kept from the movement payload, read by EntityId,
// and cleared only where the identity ends.
use super::*;
use postretro_net::wire::{WireMovementState, WireTeleportState, WireTransform};

fn movement(life_state: u8, assistable: bool) -> ComponentPayload {
    ComponentPayload::PlayerMovementState(WirePlayerMovementState {
        life_state,
        life_state_elapsed_ticks: 0,
        life_state_table_revision: 0,
        assistable,
        teleport: WireTeleportState::new(0, 0.0, postretro_net::wire::NO_WIRE_TELEPORTER, &[])
            .unwrap(),
        knockback_velocity: [0.0; 3],
        push_velocity: [0.0; 3],
        velocity: [0.0; 3],
        ground: WireGroundRef::Airborne,
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

fn pawn(
    network_id: u32,
    baseline_id: u32,
    local_player: bool,
    life_state: u8,
    assistable: bool,
) -> EntityRecord {
    EntityRecord::FullBaseline {
        assist_facts: None,
        network_id,
        baseline_id,
        last_processed_client_tick: local_player.then_some(0),
        local_player,
        entity_class: None,
        active_weapon_archetype: None,
        projectile_presentation: None,
        presentation_binding: None,
        components: vec![
            ComponentPayload::Transform(WireTransform {
                position: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            }),
            movement(life_state, assistable),
        ],
    }
}

fn snapshot(sequence: u32, records: Vec<EntityRecord>) -> SnapshotMessage {
    SnapshotMessage {
        sequence,
        server_tick: sequence + 1,
        records,
        state_schema_fingerprint: [0u8; 32],
        state_records: Vec::new(),
        faction_sentiment_record: None,
        ghost_audio: None,
        server_script_time: 0.0,
        level_tick: 0,
    }
}

fn facts(life_state: u8, assistable: bool) -> Option<RemoteAssistFacts> {
    Some(RemoteAssistFacts {
        life_state,
        assistable,
    })
}

/// A client holding remote pawn 7 (`down`, assistable) and its entity.
fn retained() -> (EntityRegistry, ClientReplication, EntityId) {
    let mut registry = EntityRegistry::new();
    let mut client = ClientReplication::new();
    let outcome = client.apply_snapshot(
        &mut registry,
        &snapshot(0, vec![pawn(7, 1, false, 1, true)]),
    );
    assert_eq!(
        outcome.ignored,
        vec![IgnoredPayload::MovementWithoutLocalComponent { network_id: 7 }],
        "no component merge happens for a remote pawn"
    );
    let entity = client.entity_for_network_id(NetworkId(7)).unwrap();
    (registry, client, entity)
}

#[test]
fn remote_pawn_assist_facts_read_back_by_entity_and_each_payload_overwrites() {
    let (mut registry, mut client, entity) = retained();
    assert_eq!(client.remote_assist_facts(entity), facts(1, true));
    assert!(
        registry
            .get_component::<postretro_foundation::PlayerMovementComponent>(entity)
            .is_err(),
        "retention constructs no movement component"
    );

    client.apply_snapshot(
        &mut registry,
        &snapshot(1, vec![pawn(7, 2, false, 0, false)]),
    );
    assert_eq!(client.remote_assist_facts(entity), facts(0, false));

    let unmapped = registry.spawn(Transform::default());
    assert_eq!(client.remote_assist_facts(unmapped), None);
}

#[test]
fn meshless_locomotion_drop_keeps_assist_facts() {
    let (_registry, mut client, entity) = retained();
    client.cache_remote_player_locomotion(
        NetworkId(7),
        Some(RemotePlayerLocomotionReference::idle_and_walk_forward(60.0)),
    );
    client.cache_remote_player_locomotion(NetworkId(7), None);
    assert!(!client.remote_player_locomotion.contains_key(&NetworkId(7)));
    assert_eq!(client.remote_assist_facts(entity), facts(1, true));
}

#[test]
fn despawn_clears_assist_facts_and_a_returning_identity_starts_empty() {
    let (mut registry, mut client, entity) = retained();
    client.apply_snapshot(
        &mut registry,
        &snapshot(
            1,
            vec![EntityRecord::Despawn {
                network_id: 7,
                tombstone_id: 4,
                reason: 0,
            }],
        ),
    );
    assert_eq!(client.remote_assist_facts(entity), None);
    assert!(client.remote_assist.get(NetworkId(7)).is_none());
}

#[test]
fn local_pawn_arming_drops_assist_facts_and_its_later_payloads_are_not_retained() {
    let (mut registry, mut client, entity) = retained();
    client.apply_snapshot(&mut registry, &snapshot(1, vec![pawn(7, 2, true, 1, true)]));
    assert_eq!(client.local_pawn_network_id(), Some(NetworkId(7)));
    assert_eq!(client.remote_assist_facts(entity), None);
    client.apply_snapshot(&mut registry, &snapshot(2, vec![pawn(7, 3, true, 1, true)]));
    assert_eq!(client.remote_assist_facts(entity), None);
}

#[test]
fn level_unload_and_demotion_clear_assist_facts() {
    let (_registry, mut client, _entity) = retained();
    let _ = client.reset_for_level_unload();
    assert!(client.remote_assist.get(NetworkId(7)).is_none());

    let (_registry, mut client, _entity) = retained();
    client.reset_for_demotion();
    assert!(client.remote_assist.get(NetworkId(7)).is_none());
}

/// The client assist step's candidate list: every retained remote pawn with
/// its mapped entity, in entity-id order, into a reused buffer.
#[test]
fn remote_assist_pawns_list_every_retained_mapped_pawn_in_entity_order() {
    let mut registry = EntityRegistry::new();
    let mut client = ClientReplication::new();
    client.apply_snapshot(
        &mut registry,
        &snapshot(
            0,
            vec![pawn(9, 1, false, 0, false), pawn(7, 1, false, 1, true)],
        ),
    );
    let mut out = vec![(EntityId::from_raw(99), facts(0, false).unwrap())];
    client.remote_assist_pawns(&mut out);
    let nine = client.entity_for_network_id(NetworkId(9)).unwrap();
    let seven = client.entity_for_network_id(NetworkId(7)).unwrap();
    let mut expected = vec![
        (nine, facts(0, false).unwrap()),
        (seven, facts(1, true).unwrap()),
    ];
    expected.sort_by_key(|(entity, _)| *entity);
    assert_eq!(out, expected, "the stale entry is cleared");
}

/// A remote pawn's assist position is the interpolation buffer's pose at the
/// render tick remote sampling used, the pose sampling writes, even on a
/// snapshot-apply frame whose registry transform holds the raw newest pose;
/// before time sync names a render tick it is the newest buffered pose.
#[test]
fn remote_assist_position_reads_the_interpolated_pose_not_the_raw_applied_one() {
    let at = |baseline_id: u32, x: f32| {
        let mut record = pawn(7, baseline_id, false, 1, true);
        if let EntityRecord::FullBaseline { components, .. } = &mut record {
            components[0] = ComponentPayload::Transform(WireTransform {
                position: [x, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            });
        }
        record
    };
    let mut registry = EntityRegistry::new();
    let mut client = ClientReplication::new();
    client.apply_snapshot(&mut registry, &snapshot(0, vec![at(1, 0.0)]));
    client.apply_snapshot(&mut registry, &snapshot(10, vec![at(2, 10.0)]));
    let entity = client.entity_for_network_id(NetworkId(7)).unwrap();
    let raw = registry
        .get_component::<postretro_entities::Transform>(entity)
        .unwrap()
        .position;
    assert_eq!(raw.x, 10.0, "the apply wrote the raw newest pose");

    let interpolated = client.remote_assist_position(entity, Some(3.0)).unwrap();
    assert!((interpolated.x - 2.0).abs() < 1e-4, "{interpolated:?}");
    client.sample_into_registry(&mut registry, 3.0, 0.0);
    let presented = registry
        .get_component::<postretro_entities::Transform>(entity)
        .unwrap()
        .position;
    assert!(
        (presented - interpolated).length() < 1e-4,
        "the pose sampling presents"
    );
    assert_eq!(
        client.remote_assist_position(entity, None).unwrap().x,
        10.0,
        "the newest buffered pose before time sync"
    );
    assert_eq!(
        client.remote_assist_position(EntityId::from_raw(999), Some(3.0)),
        None
    );
}
