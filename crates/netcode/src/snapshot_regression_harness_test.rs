//! Snapshot regressions belong to netcode because they assert wire ownership.

use std::collections::HashMap;

use glam::Vec3;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::mesh::{
    AnimationState, InterruptPolicy, MeshAnimation, MeshComponent, resolve_pending_animation_stamps,
};
use postretro_entities::data_descriptors::HealthDescriptor;
use postretro_entities::{EntityRegistry, Transform};
use postretro_foundation::{
    AirParams, CapsuleParams, FallParams, ForgivenessParams, GroundParams, PlayerMovementComponent,
    PlayerMovementDescriptor, SpeedParams,
};
use postretro_sim::impact_effects::{despawn, run_end_of_frame_removal_pass};
use postretro_sim::scripting_systems::hit_zones::HitZoneStore;
use postretro_sim::sim::update_player_animation_locomotion;

use crate::{HostCommandQueues, MovementOwners, NetworkIdAllocator, ReplicableSet};

fn player_descriptor() -> PlayerMovementDescriptor {
    PlayerMovementDescriptor {
        knockback: Default::default(),
        capsule: CapsuleParams {
            radius: 0.4,
            half_height: 0.8,
            eye_height: 0.5,
        },
        ground: GroundParams {
            speed: SpeedParams {
                walk: 7.0,
                run: 11.0,
                crouch: 3.0,
            },
            accel: 10.0,
            step_height: 0.3,
            max_slope: 45.0,
        },
        air: AirParams {
            forward_steer: 0.0,
            accel: 0.7,
            max_control_speed: 0.5,
            bunny_hop: false,
            jumps: 0,
            jump_velocity: 5.5,
            jump_ceiling: 0.0,
        },
        fall: FallParams {
            terminal_velocity: 40.0,
        },
        stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
        stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
        dash: None,
        forgiveness: Some(ForgivenessParams {
            coyote_ms: 0.0,
            jump_buffer_ms: 0.0,
        }),
        crouch: None,
        slide: None,
        view_feel: None,
    }
}

// Moved from sim/impact_effects.rs. Host snapshots must be collected after
// frame-end removals so clients cannot recreate a terminally removed entity.
#[test]
fn authoritative_snapshot_after_removal_omits_terminal_entity() {
    let mut registry = EntityRegistry::new();
    let target = registry.spawn(Transform::default());
    registry
        .set_component(
            target,
            HealthComponent::from_descriptor(&HealthDescriptor {
                max: 100.0,
                hitbox: None,
                zone_multipliers: Default::default(),
            }),
        )
        .expect("target is live");
    let mut replicable = ReplicableSet::new();
    replicable.register(target);

    despawn(&mut registry, target, None);
    run_end_of_frame_removal_pass(&mut registry, |_, _| {});

    let snapshots = crate::produce_owned_snapshots(
        &registry,
        &replicable,
        &mut NetworkIdAllocator::new(),
        &MovementOwners::new(),
        &HostCommandQueues::new(),
    );
    assert!(snapshots.is_empty());
}

// Moved from sim/determinism_tests.rs. Host-owned pawns carry no `Brain`, so
// their locomotion state must still be calibrated before netcode serializes it.
#[test]
fn host_player_locomotion_selects_walk_and_calibrates_rate_before_replication() {
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    let mut movement = PlayerMovementComponent::from_descriptor(&player_descriptor());
    movement.velocity = Vec3::new(3.0, 0.0, 0.0);
    registry.set_component(pawn, movement).unwrap();

    let mut states = HashMap::new();
    states.insert(
        "idle".to_string(),
        AnimationState {
            clip: "idle".to_string(),
            looping: true,
            crossfade_ms: 50.0,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            clip_index: Some(0),
        },
    );
    states.insert(
        "walk_forward".to_string(),
        AnimationState {
            clip: "walk".to_string(),
            looping: true,
            crossfade_ms: 50.0,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: Some(2.0),
            clip_index: Some(1),
        },
    );
    registry
        .set_component(
            pawn,
            MeshComponent::animated(
                "player-model".to_string(),
                MeshAnimation::new(states, "idle".to_string()),
            ),
        )
        .unwrap();
    resolve_pending_animation_stamps(&mut registry, 0.5);

    update_player_animation_locomotion(&mut registry, &HitZoneStore::new(), 1.0);

    let animation = registry
        .get_component::<MeshComponent>(pawn)
        .unwrap()
        .animation
        .as_ref()
        .unwrap();
    assert_eq!(animation.current_state, "walk_forward");
    assert!(
        animation.previous_state.is_some(),
        "switch uses authored crossfade"
    );
    assert!((animation.rate - 1.5).abs() <= 1.0e-6);

    let mut replicable = ReplicableSet::new();
    replicable.register(pawn);
    let mut allocator = NetworkIdAllocator::new();
    let snapshots = crate::produce_owned_snapshots(
        &registry,
        &replicable,
        &mut allocator,
        &MovementOwners::new(),
        &HostCommandQueues::new(),
    );
    assert!(snapshots[0].components.iter().any(|payload| matches!(
        payload,
        postretro_net::wire::ComponentPayload::MeshAnimationState(state)
            if state.current_state == "walk_forward"
    )));
}
