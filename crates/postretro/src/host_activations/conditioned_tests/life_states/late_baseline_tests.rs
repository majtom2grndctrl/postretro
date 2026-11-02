//! A new recipient sees an already downed teammate through the normal baseline.
//! See: context/lib/networking.md §Snapshot apply ordering

use super::*;
use crate::host_activations::conditioned_tests::receive_alternate::NoBindings;
use postretro_entities::components::mesh::{AnimationState, InterruptPolicy, MeshComponent};
use postretro_entities::data_descriptors::MeshDescriptor;
use postretro_foundation::{CrouchParams, LifeStatePosture};
use postretro_scripting_core::ir::{BakedIr, CURRENT_IR_VERSION, bind, eval_value};
use postretro_scripting_core::ir_player_scope::PlayerConditionScope;

#[test]
fn life_state_late_join_full_baseline_carries_downed_teammate_and_presents_crouched_avatar() {
    const LATE: u64 = 99;
    let mut life = LifeFixture::new(descriptor(false, false, 1, "press"), true);
    let crouch = CrouchParams {
        half_height: 0.4,
        eye_height: 0.3,
        transition_rate: 8.0,
    };
    let mut descriptors = life.ctx.data_registry.borrow().entities.clone();
    let player = descriptors
        .iter_mut()
        .find(|entity| entity.canonical_name.as_deref() == Some("player"))
        .unwrap();
    player.movement.as_mut().unwrap().crouch = Some(crouch.clone());
    player
        .life_states
        .as_mut()
        .unwrap()
        .states
        .get_mut("down")
        .unwrap()
        .movement
        .posture = LifeStatePosture::Crouch;
    player.mesh = Some(MeshDescriptor {
        model: "models/baseline-avatar/scene.gltf".into(),
        shadow_only: true,
        attachments: Default::default(),
        shadow_bias_scale: 1.0,
        animations: ["idle", "walk_forward", "crouch_idle", "crouch_forward"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    name.into(),
                    AnimationState {
                        clip: name.into(),
                        looping: true,
                        crossfade_ms: 50.0,
                        interrupt: InterruptPolicy::Smooth,
                        travel_speed: None,
                        clip_index: Some(index),
                    },
                )
            })
            .collect(),
        default_state: Some("idle".into()),
        locomotion: None,
    });
    life.ctx
        .data_registry
        .borrow_mut()
        .replace_entity_types(descriptors);
    life.fixture.descriptors = life.ctx.data_registry.borrow().entities.clone();
    {
        let mut registry = life.fixture.host.borrow_mut();
        let ComponentValue::PlayerMovement(movement) = registry
            .get_component_value_mut(life.fixture.host_actors.pawn, ComponentKind::PlayerMovement)
            .unwrap()
        else {
            unreachable!()
        };
        movement.crouch = Some(crouch);
    }
    life.zero_shooter(false);
    for _ in 0..4 {
        life.step(neutral());
    }
    let pawn = life.fixture.host_actors.pawn;
    let host_movement = |life: &LifeFixture| {
        life.fixture
            .host
            .borrow()
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone()
    };
    let pre_join = host_movement(&life);
    assert_eq!(pre_join.life_state, life.down_id);
    assert!(pre_join.life_state_elapsed_ticks > 0);
    near(pre_join.capsule.half_height, 0.4);
    assert!(matches!(
        pre_join.movement_state,
        postretro_foundation::MovementState::Crouching { .. }
    ));

    let scope = PlayerConditionScope::new(life.ctx.clone());
    let condition = bind(
        &BakedIr {
            version: CURRENT_IR_VERSION,
            output: None,
            root: IrNode::Input {
                name: "player.inLifeState:down".into(),
                owner: None,
            },
        },
        &scope,
    )
    .unwrap();
    let assert_host_down = |life: &LifeFixture| {
        scope.seed_player(
            &life.ctx.registry.borrow(),
            &life.ctx.slot_table.borrow(),
            pawn,
            Some(Seat(1)),
        );
        assert_eq!(eval_value(&condition, &scope), IrValue::Bool(true));
    };
    assert_host_down(&life);

    let mut client = NetClient::new(
        UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap(),
        (Ipv4Addr::LOCALHOST, 1).into(),
        LATE,
        Duration::from_secs(1),
        Some([0x5a; 32]),
        None,
    )
    .unwrap();
    client.set_mod_identity("test.mod".into(), "1.0.0".into());
    client.set_mod_digest(Some([0x5a; 32]));
    client.set_level_parity(Some(("test-level".into(), [0x5a; 32])));
    client.set_revealed_level(Some(postretro_net::wire::RevealedLevel::new(
        "test-level",
        None,
    )));
    client.set_connected();
    life.fixture.server.add_relay_connection(LATE, None);
    // The host keeps simulating through the handshake: player events, movement
    // and replication all run while the condition is read. Ticking stops once
    // the host admits the recipient, so no production snapshot reaches it
    // ahead of the baseline under test.
    for _ in 0..32 {
        if !life.fixture.server.is_participating(LATE) {
            life.step(neutral());
        }
        assert_host_down(&life);
        client.update_connections(Duration::from_millis(17));
        life.fixture
            .server
            .update_connections(Duration::from_millis(17));
        for packet in client.packets_to_send() {
            life.fixture.server.process_packet_from(&packet, LATE);
        }
        let _ = life.fixture.server.poll_handshakes();
        for packet in life.fixture.server.packets_to_send(LATE) {
            client.process_packet(&packet);
        }
        let _ = client.drain_control();
        if client.is_participating() && life.fixture.server.is_participating(LATE) {
            break;
        }
    }
    assert!(client.is_participating() && life.fixture.server.is_participating(LATE));
    let expected = host_movement(&life);
    assert_eq!(expected.life_state, life.down_id);
    assert!(
        expected.life_state_elapsed_ticks > pre_join.life_state_elapsed_ticks,
        "the downed clock kept advancing on the host across the handshake"
    );
    let mut tracker = ServerReplication::new();
    for _ in 0..2 {
        tracker.ingest_tick(netcode::produce_owned_snapshots_with_host_aim(
            &life.fixture.host.borrow(),
            &life.fixture.receive.as_ref().unwrap().set,
            &mut life.fixture.allocator,
            &life.fixture.owners,
            &life.fixture.queues,
            None,
            &Default::default(),
        ));
        let _ = tracker.begin_batch();
    }
    // No recipient existed while the pawn entered down and its capsule crouched.
    // The baseline is built at admission, after the host ticked across the join.
    tracker.register_client(LATE);
    let sequence = tracker.begin_batch();
    let raw = tracker.encode_in_batch(LATE, 900, sequence).unwrap();
    let snapshot = raw.clone().validate().unwrap();
    let network = life.fixture.allocator.network_id_for_entity(pawn).unwrap();
    let record = snapshot
        .records
        .iter()
        .find(|record| {
            matches!(record,
                wire::EntityRecord::FullBaseline { network_id, .. } if *network_id == network.0,
            )
        })
        .expect("the new recipient receives a FullBaseline for the existing teammate");
    let wire::EntityRecord::FullBaseline {
        components,
        local_player,
        ..
    } = record
    else {
        unreachable!()
    };
    assert!(!local_player);
    let movement = components
        .iter()
        .find_map(|payload| match payload {
            wire::ComponentPayload::PlayerMovementState(movement) => Some(movement),
            _ => None,
        })
        .unwrap();
    assert_eq!(movement.life_state, expected.life_state);
    assert_eq!(
        movement.life_state_elapsed_ticks,
        expected.life_state_elapsed_ticks
    );
    assert_eq!(
        movement.life_state_table_revision,
        expected.life_state_table_revision
    );
    near(movement.capsule_half_height, expected.capsule.half_height);
    assert!(matches!(
        movement.movement_state,
        wire::WireMovementState::Crouching { .. }
    ));

    assert!(life.fixture.server.send_snapshot(LATE, wire::encode(&raw)));
    let mut link = PacketConditioner::new(LinkConfig {
        delay: 75,
        ..LinkConfig::perfect()
    });
    link.enqueue_all(life.fixture.server.packets_to_send(LATE));
    let client_ctx = ScriptCtx::new();
    let mut endpoint = netcode::NetEndpoint::from_role(
        &netcode::NetRole::Connect {
            addr: (Ipv4Addr::LOCALHOST, 1).into(),
        },
        None,
    )
    .unwrap()
    .unwrap();
    // Host ticks from here on also replicate to the admitted recipient, but
    // only the conditioned link's packets reach this client.
    for _ in 0..8 {
        life.step(neutral());
        assert_host_down(&life);
        link.advance(17);
        for packet in link.take_ready() {
            client.process_packet(&packet);
        }
        let netcode::NetEndpoint::Client {
            replication,
            state_slots,
            prediction,
            life_state_tables,
            ..
        } = &mut endpoint
        else {
            unreachable!()
        };
        let _ = netcode::client_receive_and_apply(
            &mut client_ctx.registry.borrow_mut(),
            &mut client_ctx.slot_table.borrow_mut(),
            &Default::default(),
            &Default::default(),
            &Default::default(),
            &mut client,
            replication,
            state_slots,
            prediction,
            &life.fixture.descriptors,
            &life.fixture.zones,
            None,
            &life.fixture.world,
            &postretro_collision::EnvironmentVolumes::empty(),
            DT,
            Duration::from_secs_f32(DT),
            None,
            None,
            None,
            false,
            &mut NoBindings,
            15.0,
            Some(150_000.0),
            life_state_tables,
        );
    }
    let netcode::NetEndpoint::Client { replication, .. } = &mut endpoint else {
        unreachable!()
    };
    let remote = replication
        .entity_network_ids()
        .into_iter()
        .find_map(|(entity, id)| (id == network).then_some(entity))
        .unwrap();
    netcode::client_sample_interpolation_at(
        &mut client_ctx.registry.borrow_mut(),
        replication,
        900.0,
        15.0,
    );
    let registry = client_ctx.registry.borrow();
    let mesh = registry.get_component::<MeshComponent>(remote).unwrap();
    assert!(
        !mesh.shadow_only,
        "the teammate materializes as a visible remote avatar"
    );
    assert_eq!(
        mesh.animation.as_ref().unwrap().current_state,
        "crouch_idle"
    );
    assert!(
        !registry
            .has_component_kind(remote, ComponentKind::PlayerMovement)
            .unwrap()
    );
    assert_host_down(&life);
}
