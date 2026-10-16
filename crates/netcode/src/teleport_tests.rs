use glam::Vec3;
use postretro_collision::{EnvironmentView, EnvironmentVolumes};
use postretro_entities::{EntityRegistry, Transform};
use postretro_foundation::{PlayerMovementComponent, TeleportState};
use postretro_level_format::teleporters::{TeleportMode, TeleporterRecord, TeleportersSection};
use postretro_net::wire::{
    InputCommand, NetworkId, WireActivationInput, WireFireButtonState, WireMovementInput,
    WireTeleportState,
};

use crate::collision::CollisionWorld;
use crate::command_queue::{
    HostCommandQueues, MovementOwners, ResolutionSource, host_resolve_remote_commands_in_registry,
};
use crate::movement_state::movement_state_to_wire;
use crate::predict_reconcile_harness_test_fixtures::component;
use crate::prediction::{ClientPrediction, CorrectionClass};
use crate::reconcile::reconcile_local_pawn;

fn command(tick: u32, yaw: f32, stamp: f32) -> InputCommand {
    InputCommand {
        client_tick: tick,
        teleport_yaw_stamp: stamp,
        movement: WireMovementInput {
            wish_dir: [0.0, 0.0],
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: yaw,
            use_pressed: false,
            drop_pressed: false,
            aim_pitch: 0.0,
            firing_slot: 0,
        },
        fire_button: WireFireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
        secondary_button: WireFireButtonState {
            pressed: false,
            active: false,
        },
        activation: WireActivationInput::default(),
    }
}

fn world() -> CollisionWorld {
    let record = |source_entity| TeleporterRecord {
        source_entity,
        mode: TeleportMode::Seamless,
        enabled_on_spawn: true,
        // Empty regions are sufficient for adoption tests: they must not fire
        // again during replay, and their installed indices are still valid.
        brushes: Vec::new(),
        destination_origin: [100.0, 0.0, 0.0],
        destination_yaw: 0.0,
        landmark_origin: [0.0; 3],
        landmark_yaw: 0.0,
        arrival_sound: String::new(),
        on_teleport: String::new(),
    };
    let mut world = CollisionWorld::new();
    world.teleporters =
        postretro_physics::teleporters::Teleporters::from_section(&TeleportersSection {
            teleporters: vec![record(10), record(11)],
        });
    world
}

#[test]
fn teleport_input_stamp_survives_real_held_neutral_and_late_commands() {
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    let mut movement = component();
    movement.teleport.accumulated_yaw = 1.0;
    registry.set_component(pawn, movement).unwrap();
    let mut owners = MovementOwners::default();
    owners.set(pawn, 7);
    let mut queues = HostCommandQueues::default();
    queues.ingest(7, &command(0, 0.2, 0.4));
    queues.ingest(7, &command(1, 0.2, 0.4));
    for _ in 0..2 {
        let resolved = host_resolve_remote_commands_in_registry(&owners, &mut queues, &registry);
        assert_eq!(resolved[0].source, ResolutionSource::Real);
        assert!((resolved[0].command.movement.facing_yaw - 0.8).abs() < 1.0e-6);
    }
    let held = host_resolve_remote_commands_in_registry(&owners, &mut queues, &registry);
    assert_eq!(held[0].source, ResolutionSource::Held);
    assert!((held[0].command.movement.facing_yaw - 0.8).abs() < 1.0e-6);
    let mut neutral = held;
    for _ in 0..4 {
        neutral = host_resolve_remote_commands_in_registry(&owners, &mut queues, &registry);
    }
    assert_eq!(neutral[0].source, ResolutionSource::Neutral);
    assert!((neutral[0].command.movement.facing_yaw - 0.8).abs() < 1.0e-6);
    assert!((neutral[0].teleport_yaw_stamp - 0.4).abs() < 1.0e-6);
    let next_tick = queues.resolved_cursor(7).unwrap().wrapping_add(1);
    queues.ingest(7, &command(next_tick, 1.2, 1.0));
    queues.ingest(7, &command(next_tick.wrapping_add(1), 1.2, 1.0));
    let late = host_resolve_remote_commands_in_registry(&owners, &mut queues, &registry);
    assert_eq!(late[0].source, ResolutionSource::Real);
    assert!((late[0].command.movement.facing_yaw - 1.2).abs() < 1.0e-6);
}

#[test]
fn teleport_stamp_sanitization_rejects_invalid_frame_without_queueing() {
    let mut queues = HostCommandQueues::default();
    for stamp in [
        f32::NAN,
        f32::INFINITY,
        -std::f32::consts::PI,
        std::f32::consts::TAU,
    ] {
        assert!(!queues.ingest(7, &command(0, 0.0, stamp)));
    }
    assert!(queues.resolve_tick(7).is_none());
}

#[test]
fn teleport_same_generation_replacement_cuts_short_distance_and_keeps_ring_without_cue() {
    let world = world();
    let volumes = EnvironmentVolumes::empty();
    let environment = EnvironmentView::new(0.0, &volumes, &[]);
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    let mut movement = component();
    movement.teleport = TeleportState {
        generation: 1,
        last_teleporter: 0,
        ..Default::default()
    };
    registry.set_component(pawn, movement.clone()).unwrap();
    let mut prediction = ClientPrediction::new();
    prediction.arm(NetworkId(7), pawn);
    assert!(prediction.take_first_record());
    let (transform, movement, _) = prediction
        .predict_tick_with_events(
            command(1, 0.0, 0.0),
            (Transform::default(), movement),
            &world,
            environment,
            1.0 / 60.0,
        )
        .unwrap();
    registry.set_component(pawn, transform).unwrap();
    registry.set_component(pawn, movement.clone()).unwrap();
    let mut wire = movement_state_to_wire(&movement, 0.0);
    wire.teleport.last_teleporter = 1;
    let class = reconcile_local_pawn(
        &mut registry,
        &mut prediction,
        pawn,
        Transform {
            position: Vec3::X,
            ..transform
        },
        Some(&wire),
        Some(0),
        &world,
        environment,
        1.0 / 60.0,
    );
    assert_eq!(class, Some(CorrectionClass::Teleport));
    assert_eq!(
        prediction.history().len(),
        1,
        "generation cut retains unacked command"
    );
    let adoption = prediction.teleport_adoptions[0];
    assert!(adoption.camera_cut);
    assert!(adoption.yaw_delta.abs() < 1.0e-6);
    assert_eq!(adoption.cue_teleporter, None);
}

#[test]
fn teleport_first_record_is_silent_but_same_pawn_rebaseline_adopts_with_yaw_and_last_cue() {
    let world = world();
    let volumes = EnvironmentVolumes::empty();
    let environment = EnvironmentView::new(0.0, &volumes, &[]);
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    let movement = component();
    registry.set_component(pawn, movement.clone()).unwrap();
    let mut prediction = ClientPrediction::new();
    prediction.arm(NetworkId(7), pawn);
    let mut wire = movement_state_to_wire(&movement, 0.0);
    wire.teleport = WireTeleportState::new(1, 0.5, 0, &[0, 1]).unwrap();
    assert_eq!(
        reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            pawn,
            Transform {
                position: Vec3::X * 100.0,
                ..Default::default()
            },
            Some(&wire),
            None,
            &world,
            environment,
            1.0 / 60.0
        ),
        Some(CorrectionClass::Ordinary)
    );
    assert!(prediction.teleport_adoptions.is_empty());
    prediction.arm(NetworkId(7), pawn);
    wire.teleport = WireTeleportState::new(3, 1.0, 1, &[1]).unwrap();
    assert_eq!(
        reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            pawn,
            Transform {
                position: Vec3::X * 200.0,
                ..Default::default()
            },
            Some(&wire),
            None,
            &world,
            environment,
            1.0 / 60.0
        ),
        Some(CorrectionClass::Teleport)
    );
    assert_eq!(prediction.teleport_adoptions.len(), 1);
    let adoption = prediction.teleport_adoptions[0];
    assert!((adoption.yaw_delta - 0.5).abs() < 1.0e-6);
    assert_eq!(
        adoption.cue_teleporter,
        Some(1),
        "unseen arrivals coalesce to last cue"
    );
    let stored = &registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .teleport;
    assert_eq!(stored.arrival_latches, vec![1]);
}

#[test]
fn teleport_reconcile_refusal_restores_yaw_silently_and_unknown_indices_are_rejected() {
    let world = world();
    let volumes = EnvironmentVolumes::empty();
    let environment = EnvironmentView::new(0.0, &volumes, &[]);
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    let mut movement = component();
    movement.teleport = TeleportState {
        generation: 1,
        accumulated_yaw: 0.5,
        last_teleporter: 0,
        ..Default::default()
    };
    registry.set_component(pawn, movement.clone()).unwrap();
    let mut prediction = ClientPrediction::new();
    prediction.arm(NetworkId(7), pawn);
    prediction.take_first_record();
    let mut wire = movement_state_to_wire(&movement, 0.0);
    wire.teleport = Default::default();
    assert_eq!(
        reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            pawn,
            Transform {
                position: Vec3::X,
                ..Default::default()
            },
            Some(&wire),
            None,
            &world,
            environment,
            1.0 / 60.0
        ),
        Some(CorrectionClass::Teleport)
    );
    assert!((prediction.teleport_adoptions[0].yaw_delta + 0.5).abs() < 1.0e-6);
    assert_eq!(prediction.teleport_adoptions[0].cue_teleporter, None);
    wire.teleport = WireTeleportState::new(2, 0.5, 2, &[]).unwrap();
    assert_eq!(
        reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            pawn,
            Transform::default(),
            Some(&wire),
            None,
            &world,
            environment,
            1.0 / 60.0
        ),
        None
    );
}
