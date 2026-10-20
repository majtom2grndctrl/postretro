// Host blocking-pass tests: stop, reverse and crush reactions to players and agents.
// See: context/lib/movement.md §6 · context/lib/networking.md

use super::*;
use glam::{Quat, Vec3};
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::{KinematicMoverConfig, KinematicMoverMode};
use postretro_foundation::{
    AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor, SpeedParams,
};

struct SingleMoverPose {
    mover_id: u32,
    pose: crate::collision::moving::MoverPose,
}

impl MoverPoseSource for SingleMoverPose {
    fn pose(&self, mover_id: u32) -> Option<crate::collision::moving::MoverPose> {
        (mover_id == self.mover_id).then_some(self.pose)
    }
}

struct TwoMoverPoses {
    first: SingleMoverPose,
    second: SingleMoverPose,
}

impl MoverPoseSource for TwoMoverPoses {
    fn pose(&self, mover_id: u32) -> Option<crate::collision::moving::MoverPose> {
        self.first
            .pose(mover_id)
            .or_else(|| self.second.pose(mover_id))
    }
}

struct LeggedMoverPose {
    mover_id: u32,
    pose: crate::collision::moving::MoverPose,
    legs: Vec<crate::collision::moving::MoverTranslationLeg>,
}

impl MoverPoseSource for LeggedMoverPose {
    fn pose(&self, mover_id: u32) -> Option<crate::collision::moving::MoverPose> {
        (mover_id == self.mover_id).then_some(self.pose)
    }

    fn translation_legs(
        &self,
        mover_id: u32,
    ) -> Option<&[crate::collision::moving::MoverTranslationLeg]> {
        (mover_id == self.mover_id).then_some(self.legs.as_slice())
    }
}

fn player_movement() -> PlayerMovementComponent {
    PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
        sounds: None,
        knockback: Default::default(),
        capsule: CapsuleParams {
            radius: 0.25,
            half_height: 0.5,
            eye_height: 0.4,
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
        forgiveness: None,
        crouch: None,
        slide: None,
        swim: None,
        vault: None,
        view_feel: None,
    })
}

fn swept_wall(mover_id: u32) -> MoverCollider {
    MoverCollider::from_local_triangles(
        mover_id,
        &[
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 2.0, -1.0),
            Vec3::new(0.0, 2.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
        ],
        &[[0, 1, 2], [0, 2, 3]],
    )
    .expect("wall fixture is valid")
}

fn moving_contact_pose(mover_id: u32) -> SingleMoverPose {
    SingleMoverPose {
        mover_id,
        pose: crate::collision::moving::MoverPose {
            transform: Transform {
                position: Vec3::X,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            linear_velocity: Vec3::new(20.0, 0.0, 0.0),
            tick_delta: Vec3::new(2.0, 0.0, 0.0),
            angular_velocity: Vec3::ZERO,
            tick_rotation_delta: Quat::IDENTITY,
            carry_yaw: false,
            tick_dt: 0.1,
        },
    }
}

fn stationary_contact_pose(mover_id: u32) -> SingleMoverPose {
    SingleMoverPose {
        mover_id,
        pose: crate::collision::moving::MoverPose {
            transform: Transform::default(),
            linear_velocity: Vec3::ZERO,
            tick_delta: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            tick_rotation_delta: Quat::IDENTITY,
            carry_yaw: false,
            tick_dt: 0.1,
        },
    }
}

fn translating_rotating_crossing_pose(mover_id: u32) -> LeggedMoverPose {
    LeggedMoverPose {
        mover_id,
        pose: crate::collision::moving::MoverPose {
            transform: Transform {
                position: Vec3::X,
                rotation: Quat::from_rotation_y(std::f32::consts::PI),
                scale: Vec3::ONE,
            },
            linear_velocity: Vec3::new(5.0, 0.0, 0.0),
            tick_delta: Vec3::new(0.5, 0.0, 0.0),
            angular_velocity: Vec3::Y * (std::f32::consts::PI / 0.1),
            tick_rotation_delta: Quat::from_rotation_y(std::f32::consts::PI),
            carry_yaw: false,
            tick_dt: 0.1,
        },
        legs: vec![crate::collision::moving::MoverTranslationLeg {
            start: Vec3::new(0.5, 0.0, 0.0),
            end: Vec3::X,
            start_tick_fraction: 0.0,
            end_tick_fraction: 1.0,
        }],
    }
}

fn mover(mover_id: u32) -> KinematicMoverComponent {
    KinematicMoverComponent::new(
        mover_id,
        KinematicMoverConfig {
            waypoints: vec![Vec3::ZERO, Vec3::X],
            waypoint_names: vec!["closed".to_string(), "open".to_string()],
            speed_mps: 1.0,
            wait_ms: 0.0,
            mode: KinematicMoverMode::PingPong,
            started: true,
            spin_axis: Vec3::ZERO,
            initial_spin_rate_rad_s: 0.0,
            spin_accel_rad_s2: 0.0,
            carry_yaw: false,
        },
    )
}

#[test]
fn mover_event_kinds_map_to_their_authored_dispatch_addresses() {
    let mut mover = mover(7);
    mover.open_event = Some("door.open".to_string());
    mover.close_event = Some("door.close".to_string());
    mover.blocked_event = Some("door.blocked".to_string());
    mover.crush_event = Some("door.crush".to_string());

    assert_eq!(
        MoverEventKind::Opened.dispatch_address(&mover),
        Some("door.open")
    );
    assert_eq!(
        MoverEventKind::Closed.dispatch_address(&mover),
        Some("door.close")
    );
    assert_eq!(
        MoverEventKind::Blocked.dispatch_address(&mover),
        Some("door.blocked")
    );
    assert_eq!(
        MoverEventKind::Crushed.dispatch_address(&mover),
        Some("door.crush")
    );

    mover.crush_event = None;
    assert_eq!(MoverEventKind::Crushed.dispatch_address(&mover), None);
}

fn add_player(registry: &mut EntityRegistry, health: Option<f32>) -> EntityId {
    let player = registry.spawn(Transform {
        position: Vec3::Y,
        ..Transform::default()
    });
    registry
        .set_component(player, player_movement())
        .expect("player movement attaches");
    if let Some(current) = health {
        registry
            .set_component(
                player,
                HealthComponent {
                    max: 100.0,
                    current,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .expect("player health attaches");
    }
    player
}

fn set_player_ground(registry: &mut EntityRegistry, player: EntityId, ground: GroundRef) {
    let mut movement = registry
        .get_component::<PlayerMovementComponent>(player)
        .expect("player movement remains attached")
        .clone();
    movement.ground = ground;
    registry
        .set_component(player, movement)
        .expect("player movement updates");
}

fn add_enemy(registry: &mut EntityRegistry, health: Option<f32>) -> EntityId {
    let enemy = registry.spawn(Transform {
        position: Vec3::Y,
        ..Transform::default()
    });
    registry
        .set_component(enemy, AgentComponent::new(0.25, 1.5, 0.3, 4.0))
        .expect("agent component attaches");
    if let Some(current) = health {
        registry
            .set_component(
                enemy,
                HealthComponent {
                    max: 100.0,
                    current,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .expect("enemy health attaches");
    }
    enemy
}

fn blocking_static_wall() -> CollisionWorld {
    let points = vec![
        Vec3::new(0.1, 0.0, -1.0),
        Vec3::new(0.1, 2.0, -1.0),
        Vec3::new(0.1, 2.0, 1.0),
        Vec3::new(0.1, 0.0, 1.0),
    ];
    CollisionWorld::from_triangles_for_test(points, vec![[0, 1, 2], [0, 2, 3]])
}

#[test]
fn stop_policy_holds_on_swept_player_contact_and_clears_after_contact() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut blocking_mover = mover(mover_id);
    blocking_mover.block_policy = BlockPolicy::Stop;
    registry
        .set_component(mover_entity, blocking_mover)
        .unwrap();
    let player = add_player(&mut registry, None);

    let collider = swept_wall(mover_id);
    let static_world = CollisionWorld::new();
    let contacting_poses = moving_contact_pose(mover_id);
    let mut events = Vec::new();
    let mut blocking_state = MoverBlockingState::default();

    run_mover_blocking_pass(
        &mut registry,
        &static_world,
        std::slice::from_ref(&collider),
        &contacting_poses,
        &mut blocking_state,
        0.1,
        &mut events,
        &mut |_| {},
    );
    assert!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .blocked
    );
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);

    let mut player_transform = *registry.get_component::<Transform>(player).unwrap();
    player_transform.position.x = 10.0;
    registry.set_component(player, player_transform).unwrap();
    run_mover_blocking_pass(
        &mut registry,
        &static_world,
        std::slice::from_ref(&collider),
        &stationary_contact_pose(mover_id),
        &mut blocking_state,
        0.1,
        &mut events,
        &mut |_| {},
    );
    assert!(
        !registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .blocked
    );
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);
}

// Regression: a platform treated its own passenger as an obstruction and
// entered the replicated stop hold.
#[test]
fn grounded_player_does_not_block_its_stop_mover() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Stop;
    registry.set_component(mover_entity, mover).unwrap();
    let player = add_player(&mut registry, None);
    set_player_ground(&mut registry, player, GroundRef::Mover(mover_id));

    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();
    for _ in 0..2 {
        run_mover_blocking_pass(
            &mut registry,
            &CollisionWorld::new(),
            std::slice::from_ref(&swept_wall(mover_id)),
            &moving_contact_pose(mover_id),
            &mut state,
            0.1,
            &mut events,
            &mut |_| {},
        );
    }

    assert!(
        !registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .blocked
    );
    assert!(events.is_empty());
}

#[test]
fn grounded_player_does_not_reverse_its_mover() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Reverse;
    registry.set_component(mover_entity, mover).unwrap();
    let player = add_player(&mut registry, None);
    set_player_ground(&mut registry, player, GroundRef::Mover(mover_id));

    let mut events = Vec::new();
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&swept_wall(mover_id)),
        &moving_contact_pose(mover_id),
        &mut MoverBlockingState::default(),
        0.1,
        &mut events,
        &mut |_| {},
    );

    assert_eq!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .direction_sign,
        1
    );
    assert!(events.is_empty());
}

#[test]
fn reactive_exemption_follows_live_ground_and_only_matches_its_mover() {
    let mover_id = 42;
    let other_mover_id = 7;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut blocking_mover = mover(mover_id);
    blocking_mover.block_policy = BlockPolicy::Stop;
    registry
        .set_component(mover_entity, blocking_mover)
        .unwrap();
    let other_mover_entity = registry.spawn(Transform::default());
    let mut other_mover = mover(other_mover_id);
    other_mover.started = false;
    registry
        .set_component(other_mover_entity, other_mover)
        .unwrap();
    let player = add_player(&mut registry, None);
    let collider = swept_wall(mover_id);
    let poses = moving_contact_pose(mover_id);
    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();

    set_player_ground(&mut registry, player, GroundRef::Mover(mover_id));
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&collider),
        &poses,
        &mut state,
        0.1,
        &mut events,
        &mut |_| {},
    );
    assert!(events.is_empty());

    set_player_ground(&mut registry, player, GroundRef::World);
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&collider),
        &poses,
        &mut state,
        0.1,
        &mut events,
        &mut |_| {},
    );
    assert!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .blocked
    );
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);

    let mut blocking_mover = registry
        .get_component::<KinematicMoverComponent>(mover_entity)
        .unwrap()
        .clone();
    blocking_mover.blocked = false;
    registry
        .set_component(mover_entity, blocking_mover)
        .unwrap();
    set_player_ground(&mut registry, player, GroundRef::Mover(other_mover_id));
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&collider),
        &poses,
        &mut state,
        0.1,
        &mut events,
        &mut |_| {},
    );
    assert!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .blocked,
        "a player riding another mover remains an obstruction"
    );
}

#[test]
fn airborne_rider_grace_keeps_a_departing_player_from_stopping_then_expires() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Stop;
    registry.set_component(mover_entity, mover).unwrap();
    let player = add_player(&mut registry, None);
    let collider = swept_wall(mover_id);
    let poses = moving_contact_pose(mover_id);
    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();

    set_player_ground(&mut registry, player, GroundRef::Mover(mover_id));
    for _ in 0..2 {
        run_mover_blocking_pass(
            &mut registry,
            &CollisionWorld::new(),
            std::slice::from_ref(&collider),
            &poses,
            &mut state,
            0.1,
            &mut events,
            &mut |_| {},
        );
    }

    set_player_ground(&mut registry, player, GroundRef::Airborne);
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&collider),
        &poses,
        &mut state,
        0.1,
        &mut events,
        &mut |_| {},
    );
    assert!(events.is_empty(), "a fresh grace window skips the contact");
    assert!(
        !registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .blocked
    );

    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&collider),
        &poses,
        &mut state,
        0.051,
        &mut events,
        &mut |_| {},
    );
    assert!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .blocked,
        "the bounded airborne exemption expires after RIDER_GRACE_MS"
    );
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);
}

// Regression: an idle mover did not remember its rider before a trigger
// started it during the rider's airborne departure.
#[test]
fn idle_mover_refreshes_rider_grace_before_it_starts() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Stop;
    mover.started = false;
    registry.set_component(mover_entity, mover).unwrap();
    let player = add_player(&mut registry, None);
    let collider = swept_wall(mover_id);
    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();

    set_player_ground(&mut registry, player, GroundRef::Mover(mover_id));
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&collider),
        &stationary_contact_pose(mover_id),
        &mut state,
        0.05,
        &mut events,
        &mut |_| {},
    );

    set_player_ground(&mut registry, player, GroundRef::Airborne);
    let mut started_mover = registry
        .get_component::<KinematicMoverComponent>(mover_entity)
        .unwrap()
        .clone();
    started_mover.started = true;
    registry.set_component(mover_entity, started_mover).unwrap();
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&collider),
        &moving_contact_pose(mover_id),
        &mut state,
        0.05,
        &mut events,
        &mut |_| {},
    );

    assert!(
        !registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .blocked
    );
    assert!(events.is_empty());
}

#[test]
fn airborne_rider_grace_keeps_a_departing_player_from_reversing() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Reverse;
    registry.set_component(mover_entity, mover).unwrap();
    let player = add_player(&mut registry, None);
    let collider = swept_wall(mover_id);
    let poses = moving_contact_pose(mover_id);
    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();

    set_player_ground(&mut registry, player, GroundRef::Mover(mover_id));
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&collider),
        &poses,
        &mut state,
        0.05,
        &mut events,
        &mut |_| {},
    );
    set_player_ground(&mut registry, player, GroundRef::Airborne);
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&collider),
        &poses,
        &mut state,
        0.1,
        &mut events,
        &mut |_| {},
    );

    assert_eq!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .direction_sign,
        1
    );
    assert!(events.is_empty());

    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&collider),
        &poses,
        &mut state,
        0.051,
        &mut events,
        &mut |_| {},
    );

    assert_eq!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .direction_sign,
        -1,
        "reverse policy reacts after rider grace expires"
    );
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);
}

// Regression: transferring from mover A to B left A's grace active after
// departing B, exempting two movers at once.
#[test]
fn rider_transfer_keeps_airborne_grace_only_for_the_last_mover() {
    let first_mover_id = 42;
    let second_mover_id = 7;
    let mut registry = EntityRegistry::new();
    let first_mover_entity = registry.spawn(Transform::default());
    let mut first_mover = mover(first_mover_id);
    first_mover.block_policy = BlockPolicy::Stop;
    first_mover.started = false;
    registry
        .set_component(first_mover_entity, first_mover)
        .unwrap();
    let second_mover_entity = registry.spawn(Transform::default());
    let mut second_mover = mover(second_mover_id);
    second_mover.block_policy = BlockPolicy::Stop;
    second_mover.started = false;
    registry
        .set_component(second_mover_entity, second_mover)
        .unwrap();
    let player = add_player(&mut registry, None);
    let first_collider = swept_wall(first_mover_id);
    let second_collider = swept_wall(second_mover_id);
    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();

    set_player_ground(&mut registry, player, GroundRef::Mover(first_mover_id));
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        &[first_collider, second_collider],
        &stationary_contact_pose(first_mover_id),
        &mut state,
        0.05,
        &mut events,
        &mut |_| {},
    );
    set_player_ground(&mut registry, player, GroundRef::Mover(second_mover_id));
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        &[swept_wall(first_mover_id), swept_wall(second_mover_id)],
        &stationary_contact_pose(first_mover_id),
        &mut state,
        0.05,
        &mut events,
        &mut |_| {},
    );

    for mover_entity in [first_mover_entity, second_mover_entity] {
        let mut started_mover = registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .clone();
        started_mover.started = true;
        registry.set_component(mover_entity, started_mover).unwrap();
    }
    set_player_ground(&mut registry, player, GroundRef::Airborne);
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        &[swept_wall(first_mover_id), swept_wall(second_mover_id)],
        &TwoMoverPoses {
            first: moving_contact_pose(first_mover_id),
            second: moving_contact_pose(second_mover_id),
        },
        &mut state,
        0.05,
        &mut events,
        &mut |_| {},
    );

    assert!(
        registry
            .get_component::<KinematicMoverComponent>(first_mover_entity)
            .unwrap()
            .blocked,
        "the earlier mover is a genuine obstruction after transfer"
    );
    assert!(
        !registry
            .get_component::<KinematicMoverComponent>(second_mover_entity)
            .unwrap()
            .blocked,
        "only the last ridden mover keeps grace"
    );
    assert_eq!(events, vec![(MoverEventKind::Blocked, first_mover_id)]);
}

#[test]
fn airborne_rider_grace_does_not_exempt_a_different_mover() {
    let riding_mover_id = 42;
    let other_mover_id = 7;
    let mut registry = EntityRegistry::new();
    let riding_mover_entity = registry.spawn(Transform::default());
    let mut riding_mover = mover(riding_mover_id);
    riding_mover.block_policy = BlockPolicy::Stop;
    registry
        .set_component(riding_mover_entity, riding_mover)
        .unwrap();
    let other_mover_entity = registry.spawn(Transform::default());
    let mut other_mover = mover(other_mover_id);
    other_mover.block_policy = BlockPolicy::Stop;
    registry
        .set_component(other_mover_entity, other_mover)
        .unwrap();
    let player = add_player(&mut registry, None);
    let riding_collider = swept_wall(riding_mover_id);
    let other_collider = swept_wall(other_mover_id);
    let riding_poses = moving_contact_pose(riding_mover_id);
    let other_poses = moving_contact_pose(other_mover_id);
    let poses = TwoMoverPoses {
        first: riding_poses,
        second: other_poses,
    };
    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();

    set_player_ground(&mut registry, player, GroundRef::Mover(riding_mover_id));
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        &[riding_collider, other_collider],
        &poses,
        &mut state,
        0.05,
        &mut events,
        &mut |_| {},
    );
    events.clear();
    let mut other_mover = registry
        .get_component::<KinematicMoverComponent>(other_mover_entity)
        .unwrap()
        .clone();
    other_mover.blocked = false;
    registry
        .set_component(other_mover_entity, other_mover)
        .unwrap();

    set_player_ground(&mut registry, player, GroundRef::Airborne);
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        &[swept_wall(riding_mover_id), swept_wall(other_mover_id)],
        &poses,
        &mut state,
        0.05,
        &mut events,
        &mut |_| {},
    );

    assert!(
        !registry
            .get_component::<KinematicMoverComponent>(riding_mover_entity)
            .unwrap()
            .blocked
    );
    assert!(
        registry
            .get_component::<KinematicMoverComponent>(other_mover_entity)
            .unwrap()
            .blocked
    );
    assert_eq!(events, vec![(MoverEventKind::Blocked, other_mover_id)]);
}

#[test]
fn airborne_rider_grace_does_not_suppress_a_ceiling_crush() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 10.0;
    registry.set_component(mover_entity, mover).unwrap();
    let player = add_player(&mut registry, Some(100.0));
    let collider = swept_wall(mover_id);
    let poses = moving_contact_pose(mover_id);
    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();

    set_player_ground(&mut registry, player, GroundRef::Mover(mover_id));
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&collider),
        &poses,
        &mut state,
        0.05,
        &mut events,
        &mut |_| {},
    );
    set_player_ground(&mut registry, player, GroundRef::Airborne);
    run_mover_blocking_pass(
        &mut registry,
        &blocking_static_wall(),
        std::slice::from_ref(&collider),
        &poses,
        &mut state,
        0.05,
        &mut events,
        &mut |_| {},
    );

    let health = registry
        .get_component::<HealthComponent>(player)
        .unwrap()
        .current;
    assert!(
        (health - 90.0).abs() <= f32::EPSILON,
        "expected 90 health after one crush hit, got {health}"
    );
    assert_eq!(events, vec![(MoverEventKind::Crushed, mover_id)]);
}

// Regression: policy evaluation on an inactive pose could restart a
// stationary reverse mover merely because an entity overlapped it.
#[test]
fn inactive_reverse_contact_does_not_restart_the_mover() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Reverse;
    mover.started = false;
    registry.set_component(mover_entity, mover).unwrap();
    add_player(&mut registry, None);

    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&swept_wall(mover_id)),
        &stationary_contact_pose(mover_id),
        &mut state,
        0.1,
        &mut events,
        &mut |_| {},
    );

    let mover = registry
        .get_component::<KinematicMoverComponent>(mover_entity)
        .unwrap();
    assert!(!mover.started);
    assert_eq!(mover.direction_sign, 1);
    assert!(events.is_empty());
}

// Regression: an idle crusher damaged overlapping victims without any
// mover motion to produce a crush contact.
#[test]
fn inactive_crush_contact_deals_no_damage() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.started = false;
    mover.crush_damage = 10.0;
    registry.set_component(mover_entity, mover).unwrap();
    let player = add_player(&mut registry, Some(100.0));

    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();
    run_mover_blocking_pass(
        &mut registry,
        &blocking_static_wall(),
        std::slice::from_ref(&swept_wall(mover_id)),
        &stationary_contact_pose(mover_id),
        &mut state,
        0.1,
        &mut events,
        &mut |_| {},
    );

    assert_eq!(
        registry
            .get_component::<HealthComponent>(player)
            .unwrap()
            .current,
        100.0
    );
    assert!(events.is_empty());
}

#[test]
fn grounded_player_is_crushed_when_static_geometry_blocks_the_push() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.completed = true;
    mover.crush_damage = 10.0;
    registry.set_component(mover_entity, mover).unwrap();
    let player = add_player(&mut registry, Some(100.0));
    set_player_ground(&mut registry, player, GroundRef::Mover(mover_id));

    run_mover_blocking_pass(
        &mut registry,
        &blocking_static_wall(),
        std::slice::from_ref(&swept_wall(mover_id)),
        &moving_contact_pose(mover_id),
        &mut MoverBlockingState::default(),
        0.1,
        &mut Vec::new(),
        &mut |_| {},
    );

    assert_eq!(
        registry
            .get_component::<HealthComponent>(player)
            .unwrap()
            .current,
        90.0
    );
}

// Regression: entering a completed hold cleared the cadence table, so a
// continuously pinned victim received an incorrect fresh first hit.
#[test]
fn completed_crusher_continues_existing_cadence_without_resetting() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 10.0;
    mover.crush_interval_ms = 100.0;
    registry.set_component(mover_entity, mover).unwrap();
    let player = add_player(&mut registry, Some(100.0));
    let collider = swept_wall(mover_id);
    let mut blocking_state = MoverBlockingState::default();
    let mut events = Vec::new();

    run_mover_blocking_pass(
        &mut registry,
        &blocking_static_wall(),
        std::slice::from_ref(&collider),
        &moving_contact_pose(mover_id),
        &mut blocking_state,
        0.05,
        &mut events,
        &mut |_| {},
    );
    let mut mover = registry
        .get_component::<KinematicMoverComponent>(mover_entity)
        .unwrap()
        .clone();
    mover.completed = true;
    registry.set_component(mover_entity, mover).unwrap();

    run_mover_blocking_pass(
        &mut registry,
        &blocking_static_wall(),
        std::slice::from_ref(&collider),
        &stationary_contact_pose(mover_id),
        &mut blocking_state,
        0.05,
        &mut events,
        &mut |_| {},
    );
    assert_eq!(
        registry
            .get_component::<HealthComponent>(player)
            .unwrap()
            .current,
        90.0,
        "the stationary hold must not manufacture a fresh first hit"
    );

    run_mover_blocking_pass(
        &mut registry,
        &blocking_static_wall(),
        std::slice::from_ref(&collider),
        &stationary_contact_pose(mover_id),
        &mut blocking_state,
        0.05,
        &mut events,
        &mut |_| {},
    );
    assert_eq!(
        registry
            .get_component::<HealthComponent>(player)
            .unwrap()
            .current,
        80.0
    );
}

// Regression: timer expiry changed phase after the driver had published a
// zero pose, so the close command bypassed blocking for one tick.
#[test]
fn newly_expired_auto_close_uses_closeward_preview_for_blocking() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform {
        position: Vec3::X,
        ..Transform::default()
    });
    let mut mover = mover(mover_id);
    mover.mode = KinematicMoverMode::Once;
    mover.segment_index = 1;
    mover.completed = true;
    mover.block_policy = BlockPolicy::Stop;
    registry.set_component(mover_entity, mover).unwrap();
    let player = add_player(&mut registry, None);
    let mut player_transform = *registry.get_component::<Transform>(player).unwrap();
    player_transform.position.x = 0.5;
    registry.set_component(player, player_transform).unwrap();

    let mut closing = registry
        .get_component::<KinematicMoverComponent>(mover_entity)
        .unwrap()
        .clone();
    super::super::travel_toward_closed_terminus(&mut closing);
    registry.set_component(mover_entity, closing).unwrap();
    let published_zero_pose = SingleMoverPose {
        mover_id,
        pose: crate::collision::moving::MoverPose {
            transform: Transform {
                position: Vec3::X,
                ..Transform::default()
            },
            linear_velocity: Vec3::ZERO,
            tick_delta: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            tick_rotation_delta: Quat::IDENTITY,
            carry_yaw: false,
            tick_dt: 0.5,
        },
    };

    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&swept_wall(mover_id)),
        &published_zero_pose,
        &mut MoverBlockingState::default(),
        0.5,
        &mut Vec::new(),
        &mut |_| {},
    );

    assert!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .blocked
    );
    assert_eq!(
        registry
            .get_component::<Transform>(mover_entity)
            .unwrap()
            .position,
        Vec3::X,
        "policy preview must not move the authoritative transform"
    );
}

// Regression: a completed auto-close hold could reassert `blocked` from a
// stationary overlap after the shared driver had cleared the stale flag.
#[test]
fn completed_stationary_stop_contact_does_not_reassert_blocked() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Stop;
    mover.completed = true;
    registry.set_component(mover_entity, mover).unwrap();
    add_player(&mut registry, None);

    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&swept_wall(mover_id)),
        &stationary_contact_pose(mover_id),
        &mut state,
        0.1,
        &mut events,
        &mut |_| {},
    );

    assert!(
        !registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .blocked
    );
    assert!(events.is_empty());
}

// Regression: radius inflation reached behind and beside a mover even
// though its leading face could not contact those capsules next tick.
#[test]
fn prospective_contact_extends_only_along_the_leading_sweep() {
    let mover_id = 42;
    let collider = swept_wall(mover_id);
    let poses = moving_contact_pose(mover_id);
    let mut prospective_pose = poses.pose;
    prospective_pose.transform.position += prospective_pose.tick_delta;
    let capsule = Capsule::new_y(0.5, 0.25);

    assert!(
        leading_mover_contact_penetration(
            &collider,
            &poses,
            Some(prospective_pose),
            Vec3::new(2.5, 1.0, 0.0),
            &capsule,
        )
        .is_some(),
        "the next leading sweep remains conservative"
    );
    assert!(
        leading_mover_contact_penetration(
            &collider,
            &poses,
            Some(prospective_pose),
            Vec3::new(-2.0, 1.0, 0.0),
            &capsule,
        )
        .is_none(),
        "a capsule behind the sweep must not react"
    );
    assert!(
        leading_mover_contact_penetration(
            &collider,
            &poses,
            Some(prospective_pose),
            Vec3::new(1.0, 1.0, 2.5),
            &capsule,
        )
        .is_none(),
        "a capsule beside the sweep must not react"
    );
}

// Regression: a ping-pong tick with zero net delta could cross a capsule
// on an intermediate leg without leaving any sweep in the published pose.
#[test]
fn multi_endpoint_tick_preserves_player_and_enemy_swept_contacts() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform {
        position: Vec3::new(-2.0, 0.0, 0.0),
        ..Transform::default()
    });
    let mut mover = mover(mover_id);
    mover.waypoints = vec![Vec3::new(-2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0)];
    mover.speed_mps = 20.0;
    registry.set_component(mover_entity, mover).unwrap();
    let mut poses = super::super::MoverTickStateTable::default();
    super::super::run_kinematic_mover_tick(&mut registry, &mut poses, 0.4);
    assert!(poses.pose(mover_id).unwrap().tick_delta.length() < f32::EPSILON);

    let collider = swept_wall(mover_id);
    let player_capsule = Capsule::new_y(0.5, 0.25);
    let enemy_capsule = Capsule::new_y(0.75, 0.25);

    assert!(
        leading_mover_contact_penetration(
            &collider,
            &poses,
            None,
            Vec3::new(0.0, 1.0, 0.0),
            &player_capsule,
        )
        .is_some()
    );
    assert!(
        leading_mover_contact_penetration(
            &collider,
            &poses,
            None,
            Vec3::new(0.0, 1.0, 0.0),
            &enemy_capsule,
        )
        .is_some()
    );
}

// Regression: translation-leg sweeps erased the coincident angular
// interval, missing a rotating blade that crossed and ended clear.
#[test]
fn translating_rotating_crossing_blocks_stop_policy() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Stop;
    registry.set_component(mover_entity, mover).unwrap();
    add_enemy(&mut registry, None);

    let mut events = Vec::new();
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&swept_wall(mover_id)),
        &translating_rotating_crossing_pose(mover_id),
        &mut MoverBlockingState::default(),
        0.1,
        &mut events,
        &mut |_| {},
    );

    assert!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .blocked
    );
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);
}

// Regression: the same erased angular interval bypassed reverse policy.
#[test]
fn translating_rotating_crossing_reverses_approaching_policy() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Reverse;
    registry.set_component(mover_entity, mover).unwrap();
    add_enemy(&mut registry, None);

    let mut events = Vec::new();
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&swept_wall(mover_id)),
        &translating_rotating_crossing_pose(mover_id),
        &mut MoverBlockingState::default(),
        0.1,
        &mut events,
        &mut |_| {},
    );

    assert_eq!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .direction_sign,
        -1
    );
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);
}

// Regression: the same erased angular interval bypassed crusher damage.
#[test]
fn translating_rotating_crossing_lands_crush_damage() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 10.0;
    registry.set_component(mover_entity, mover).unwrap();
    let enemy = add_enemy(&mut registry, Some(100.0));
    let mut events = Vec::new();
    let mut impact_evaluations = 0;

    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&swept_wall(mover_id)),
        &translating_rotating_crossing_pose(mover_id),
        &mut MoverBlockingState::default(),
        0.1,
        &mut events,
        &mut |_| impact_evaluations += 1,
    );

    assert_eq!(
        registry
            .get_component::<HealthComponent>(enemy)
            .unwrap()
            .current,
        90.0
    );
    assert_eq!(impact_evaluations, 1);
    assert_eq!(events, vec![(MoverEventKind::Crushed, mover_id)]);
}

// Regression: a prospective stop contact cleared on the following
// zero-motion tick, allowing the mover to advance every other tick.
#[test]
fn prospective_stop_hold_persists_until_the_leading_sweep_clears() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform {
        position: Vec3::new(-1.0, 0.0, 0.0),
        ..Transform::default()
    });
    let mut mover = mover(mover_id);
    mover.waypoints = vec![Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)];
    mover.speed_mps = 20.0;
    mover.block_policy = BlockPolicy::Stop;
    registry.set_component(mover_entity, mover).unwrap();
    let player = add_player(&mut registry, None);
    let collider = swept_wall(mover_id);
    let mut poses = super::super::MoverTickStateTable::default();
    let mut events = Vec::new();

    super::super::run_kinematic_mover_tick(&mut registry, &mut poses, 0.025);
    let held_position = registry
        .get_component::<Transform>(mover_entity)
        .unwrap()
        .position;
    {
        let (pose_source, blocking_state) = poses.split_for_blocking();
        run_mover_blocking_pass(
            &mut registry,
            &CollisionWorld::new(),
            std::slice::from_ref(&collider),
            &pose_source,
            blocking_state,
            0.025,
            &mut events,
            &mut |_| {},
        );
    }
    assert!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .blocked
    );

    super::super::run_kinematic_mover_tick(&mut registry, &mut poses, 0.025);
    assert_eq!(
        registry
            .get_component::<Transform>(mover_entity)
            .unwrap()
            .position,
        held_position
    );
    {
        let (pose_source, blocking_state) = poses.split_for_blocking();
        run_mover_blocking_pass(
            &mut registry,
            &CollisionWorld::new(),
            std::slice::from_ref(&collider),
            &pose_source,
            blocking_state,
            0.025,
            &mut events,
            &mut |_| {},
        );
    }
    assert!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .blocked
    );

    let mut player_transform = *registry.get_component::<Transform>(player).unwrap();
    player_transform.position.x = 10.0;
    registry.set_component(player, player_transform).unwrap();
    {
        let (pose_source, blocking_state) = poses.split_for_blocking();
        run_mover_blocking_pass(
            &mut registry,
            &CollisionWorld::new(),
            std::slice::from_ref(&collider),
            &pose_source,
            blocking_state,
            0.025,
            &mut events,
            &mut |_| {},
        );
    }
    assert!(
        !registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap()
            .blocked
    );

    super::super::run_kinematic_mover_tick(&mut registry, &mut poses, 0.025);
    assert!(
        registry
            .get_component::<Transform>(mover_entity)
            .unwrap()
            .position
            .x
            > held_position.x
    );
}

#[test]
fn reverse_policy_reverses_approaching_contact_once_and_ignores_receding() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Reverse;
    mover.target_segment = Some(1);
    registry
        .set_component(mover_entity, mover)
        .expect("mover attaches");
    add_player(&mut registry, None);

    let collider = swept_wall(mover_id);
    let static_world = CollisionWorld::new();
    let poses = moving_contact_pose(mover_id);
    let mut blocking_state = MoverBlockingState::default();
    let mut events = Vec::new();

    run_mover_blocking_pass(
        &mut registry,
        &static_world,
        std::slice::from_ref(&collider),
        &poses,
        &mut blocking_state,
        0.1,
        &mut events,
        &mut |_| {},
    );
    let reversed = registry
        .get_component::<KinematicMoverComponent>(mover_entity)
        .expect("mover remains live");
    assert_eq!(reversed.direction_sign, -1);
    assert_eq!(reversed.target_segment, None);
    assert!(!reversed.blocked);
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);

    run_mover_blocking_pass(
        &mut registry,
        &static_world,
        std::slice::from_ref(&collider),
        &poses,
        &mut blocking_state,
        0.1,
        &mut events,
        &mut |_| {},
    );
    let receding = registry
        .get_component::<KinematicMoverComponent>(mover_entity)
        .expect("mover remains live");
    assert_eq!(receding.direction_sign, -1);
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);
}

#[test]
fn reverse_policy_without_a_path_degrades_to_the_replicated_stop_hold() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Reverse;
    mover.waypoints.truncate(1);
    mover.waypoint_names.truncate(1);
    registry
        .set_component(mover_entity, mover)
        .expect("mover attaches");
    add_player(&mut registry, None);

    let collider = swept_wall(mover_id);
    let static_world = CollisionWorld::new();
    let poses = moving_contact_pose(mover_id);
    let mut blocking_state = MoverBlockingState::default();
    let mut events = Vec::new();

    run_mover_blocking_pass(
        &mut registry,
        &static_world,
        std::slice::from_ref(&collider),
        &poses,
        &mut blocking_state,
        0.1,
        &mut events,
        &mut |_| {},
    );

    assert!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .expect("mover remains live")
            .blocked
    );
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);
}

#[test]
fn grounded_player_riding_crusher_is_unharmed_when_the_push_is_clear() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 10.0;
    registry
        .set_component(mover_entity, mover)
        .expect("mover attaches");
    let player = add_player(&mut registry, Some(100.0));
    set_player_ground(&mut registry, player, GroundRef::Mover(mover_id));

    let collider = swept_wall(mover_id);
    let clear_world = CollisionWorld::new();
    let poses = moving_contact_pose(mover_id);
    let mut blocking_state = MoverBlockingState::default();
    let mut events = Vec::new();
    let mut impact_evaluations = 0;

    run_mover_blocking_pass(
        &mut registry,
        &clear_world,
        std::slice::from_ref(&collider),
        &poses,
        &mut blocking_state,
        0.1,
        &mut events,
        &mut |_| impact_evaluations += 1,
    );

    assert!(
        (registry
            .get_component::<HealthComponent>(player)
            .expect("player health remains attached")
            .current
            - 100.0)
            .abs()
            < f32::EPSILON
    );
    assert_eq!(impact_evaluations, 0);
    assert!(events.is_empty());
}

#[test]
fn crush_policy_emits_no_impact_or_event_without_health() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 10.0;
    registry.set_component(mover_entity, mover).unwrap();
    add_enemy(&mut registry, None);
    let mut events = Vec::new();
    let mut impact_evaluations = 0;

    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&swept_wall(mover_id)),
        &moving_contact_pose(mover_id),
        &mut MoverBlockingState::default(),
        0.1,
        &mut events,
        &mut |_| impact_evaluations += 1,
    );

    assert_eq!(impact_evaluations, 0);
    assert!(events.is_empty());
    assert!(registry.take_impact_dispatches().is_empty());
}

#[test]
fn zero_damage_crush_emits_no_impact_or_event() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 0.0;
    registry.set_component(mover_entity, mover).unwrap();
    let enemy = add_enemy(&mut registry, Some(100.0));
    let mut events = Vec::new();
    let mut impact_evaluations = 0;

    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&swept_wall(mover_id)),
        &moving_contact_pose(mover_id),
        &mut MoverBlockingState::default(),
        0.1,
        &mut events,
        &mut |_| impact_evaluations += 1,
    );

    assert_eq!(
        registry
            .get_component::<HealthComponent>(enemy)
            .unwrap()
            .current,
        100.0
    );
    assert_eq!(impact_evaluations, 0);
    assert!(events.is_empty());
    assert!(registry.take_impact_dispatches().is_empty());
}

#[test]
fn crush_policy_cadences_per_player_and_continues_after_zero_health() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 10.0;
    mover.crush_interval_ms = 100.0;
    registry
        .set_component(mover_entity, mover)
        .expect("mover attaches");
    let first_player = add_player(&mut registry, Some(15.0));

    let collider = swept_wall(mover_id);
    let static_world = blocking_static_wall();
    let poses = moving_contact_pose(mover_id);
    let mut blocking_state = MoverBlockingState::default();
    let mut events = Vec::new();
    let mut impact_health_after = Vec::new();

    let mut run_pass = |registry: &mut EntityRegistry| {
        run_mover_blocking_pass(
            registry,
            &static_world,
            std::slice::from_ref(&collider),
            &poses,
            &mut blocking_state,
            0.05,
            &mut events,
            &mut |registry| {
                impact_health_after.extend(
                    registry
                        .take_impact_dispatches()
                        .into_iter()
                        .map(|dispatch| dispatch.health_after),
                );
            },
        );
    };

    run_pass(&mut registry); // First pinned player is hit immediately.
    let second_player = add_player(&mut registry, Some(15.0));
    run_pass(&mut registry); // The second player's first pinned tick is independent.
    run_pass(&mut registry); // First player's 100 ms cadence is due.
    run_pass(&mut registry); // Second player's 100 ms cadence is due.
    run_pass(&mut registry); // First player is hit again despite being at zero HP.

    assert!(
        registry
            .get_component::<HealthComponent>(first_player)
            .expect("first player remains attached")
            .current
            .abs()
            < f32::EPSILON
    );
    assert!(
        registry
            .get_component::<HealthComponent>(second_player)
            .expect("second player remains attached")
            .current
            .abs()
            < f32::EPSILON
    );
    assert_eq!(impact_health_after.len(), 5);
    assert!(
        impact_health_after
            .last()
            .is_some_and(|health_after| *health_after < 0.0),
        "the final hit must retain E16's raw overkill fact"
    );
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| *kind == MoverEventKind::Crushed)
            .count(),
        5
    );
}

// Pin P11: each crushed actor is its own edge, so each is its own sound.
#[test]
fn crushing_two_actors_on_one_tick_emits_two_crush_edges() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 10.0;
    mover.crush_interval_ms = 100.0;
    registry
        .set_component(mover_entity, mover)
        .expect("mover attaches");
    add_player(&mut registry, Some(50.0));
    add_player(&mut registry, Some(50.0));

    let collider = swept_wall(mover_id);
    let mut events = Vec::new();
    run_mover_blocking_pass(
        &mut registry,
        &blocking_static_wall(),
        std::slice::from_ref(&collider),
        &moving_contact_pose(mover_id),
        &mut MoverBlockingState::default(),
        0.05,
        &mut events,
        &mut |registry| {
            registry.take_impact_dispatches();
        },
    );

    assert_eq!(
        events,
        vec![
            (MoverEventKind::Crushed, mover_id),
            (MoverEventKind::Crushed, mover_id)
        ],
    );
}

#[test]
fn stop_policy_holds_on_swept_enemy_contact_without_reemitting() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Stop;
    registry.set_component(mover_entity, mover).unwrap();
    add_enemy(&mut registry, None);

    let collider = swept_wall(mover_id);
    let static_world = CollisionWorld::new();
    let poses = moving_contact_pose(mover_id);
    let mut blocking_state = MoverBlockingState::default();
    let mut events = Vec::new();

    for _ in 0..2 {
        run_mover_blocking_pass(
            &mut registry,
            &static_world,
            std::slice::from_ref(&collider),
            &poses,
            &mut blocking_state,
            0.1,
            &mut events,
            &mut |_| {},
        );
    }

    assert!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .expect("mover remains live")
            .blocked
    );
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);
}

#[test]
fn reverse_policy_reverses_approaching_enemy_contact_once() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Reverse;
    mover.target_segment = Some(1);
    registry
        .set_component(mover_entity, mover)
        .expect("mover attaches");
    add_enemy(&mut registry, None);

    let collider = swept_wall(mover_id);
    let static_world = CollisionWorld::new();
    let poses = moving_contact_pose(mover_id);
    let mut blocking_state = MoverBlockingState::default();
    let mut events = Vec::new();

    for _ in 0..2 {
        run_mover_blocking_pass(
            &mut registry,
            &static_world,
            std::slice::from_ref(&collider),
            &poses,
            &mut blocking_state,
            0.1,
            &mut events,
            &mut |_| {},
        );
    }

    let reversed = registry
        .get_component::<KinematicMoverComponent>(mover_entity)
        .expect("mover remains live");
    assert_eq!(reversed.direction_sign, -1);
    assert_eq!(reversed.target_segment, None);
    assert!(!reversed.blocked);
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);
}

#[test]
fn reverse_policy_reacts_once_when_two_enemies_contact_in_one_pass() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Reverse;
    registry
        .set_component(mover_entity, mover)
        .expect("mover attaches");
    add_enemy(&mut registry, None);
    add_enemy(&mut registry, None);

    let collider = swept_wall(mover_id);
    let static_world = CollisionWorld::new();
    let poses = moving_contact_pose(mover_id);
    let mut blocking_state = MoverBlockingState::default();
    let mut events = Vec::new();

    run_mover_blocking_pass(
        &mut registry,
        &static_world,
        std::slice::from_ref(&collider),
        &poses,
        &mut blocking_state,
        0.1,
        &mut events,
        &mut |_| {},
    );

    assert_eq!(
        registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .expect("mover remains live")
            .direction_sign,
        -1
    );
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);
}

#[test]
fn crush_policy_damages_enemy_on_overlap_and_continues_after_death_latch() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 10.0;
    mover.crush_interval_ms = 100.0;
    registry
        .set_component(mover_entity, mover)
        .expect("mover attaches");
    let enemy = add_enemy(&mut registry, Some(15.0));

    let collider = swept_wall(mover_id);
    let static_world = CollisionWorld::new();
    let poses = moving_contact_pose(mover_id);
    let mut blocking_state = MoverBlockingState::default();
    let mut events = Vec::new();
    let mut impact_health_after = Vec::new();
    let mut run_pass = |registry: &mut EntityRegistry| {
        run_mover_blocking_pass(
            registry,
            &static_world,
            std::slice::from_ref(&collider),
            &poses,
            &mut blocking_state,
            0.05,
            &mut events,
            &mut |registry| {
                impact_health_after.extend(
                    registry
                        .take_impact_dispatches()
                        .into_iter()
                        .map(|dispatch| dispatch.health_after),
                );
            },
        );
    };

    run_pass(&mut registry); // First overlap hit lands immediately.
    run_pass(&mut registry);
    run_pass(&mut registry); // The second hit reaches zero HP.
    postretro_sim::scripting_systems::health::sweep_deaths_for_test(&mut registry);
    run_pass(&mut registry);
    run_pass(&mut registry); // The latched enemy still receives overkill damage.

    let health = registry
        .get_component::<HealthComponent>(enemy)
        .expect("enemy health remains until an authored despawn");
    assert!(
        health.death_handled,
        "the existing death sweep owns the latch"
    );
    assert!(health.current.abs() < f32::EPSILON);
    assert_eq!(impact_health_after.len(), 3);
    assert!(
        impact_health_after
            .last()
            .is_some_and(|health_after| *health_after < 0.0),
        "damage after the death latch must preserve E16's overkill fact"
    );
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| *kind == MoverEventKind::Crushed)
            .count(),
        3
    );
}

#[test]
fn displace_policy_ignores_enemy_contacts() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    registry
        .set_component(mover_entity, mover(mover_id))
        .expect("mover attaches");
    let enemy = add_enemy(&mut registry, Some(100.0));

    let collider = swept_wall(mover_id);
    let static_world = CollisionWorld::new();
    let poses = moving_contact_pose(mover_id);
    let mut blocking_state = MoverBlockingState::default();
    let mut events = Vec::new();

    run_mover_blocking_pass(
        &mut registry,
        &static_world,
        std::slice::from_ref(&collider),
        &poses,
        &mut blocking_state,
        0.1,
        &mut events,
        &mut |_| {},
    );

    assert!(
        !registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .expect("mover remains live")
            .blocked
    );
    assert!(
        (registry
            .get_component::<HealthComponent>(enemy)
            .expect("enemy health remains attached")
            .current
            - 100.0)
            .abs()
            < f32::EPSILON
    );
    assert!(events.is_empty());
    assert!(registry.take_impact_dispatches().is_empty());
}

#[test]
fn crush_policy_restarts_enemy_cadence_after_unpin() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 10.0;
    mover.crush_interval_ms = 100.0;
    registry
        .set_component(mover_entity, mover)
        .expect("mover attaches");
    let enemy = add_enemy(&mut registry, Some(100.0));

    let collider = swept_wall(mover_id);
    let static_world = CollisionWorld::new();
    let poses = moving_contact_pose(mover_id);
    let mut blocking_state = MoverBlockingState::default();
    let mut events = Vec::new();
    let mut run_pass = |registry: &mut EntityRegistry| {
        run_mover_blocking_pass(
            registry,
            &static_world,
            std::slice::from_ref(&collider),
            &poses,
            &mut blocking_state,
            0.05,
            &mut events,
            &mut |_| {},
        );
    };

    run_pass(&mut registry); // First overlap hit lands immediately.
    let mut transform = *registry
        .get_component::<Transform>(enemy)
        .expect("enemy transform remains attached");
    transform.position.x = 10.0;
    registry
        .set_component(enemy, transform)
        .expect("enemy unpins");
    run_pass(&mut registry);
    transform.position.x = 0.0;
    registry
        .set_component(enemy, transform)
        .expect("enemy repins");
    run_pass(&mut registry);

    assert!(
        (registry
            .get_component::<HealthComponent>(enemy)
            .expect("enemy health remains attached")
            .current
            - 80.0)
            .abs()
            < f32::EPSILON,
        "a re-pinned enemy starts a fresh first-hit cadence"
    );
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| *kind == MoverEventKind::Crushed)
            .count(),
        2
    );
}
#[test]
fn held_world_participation_excludes_blockers_and_crush_victims_until_release() {
    use postretro_entities::components::inventory::Inventory;
    for policy in [BlockPolicy::Stop, BlockPolicy::Reverse, BlockPolicy::Crush] {
        let mover_id = 42;
        let mut registry = EntityRegistry::new();
        let holder = registry.spawn(Transform::default());
        let mover_entity = registry.spawn(Transform::default());
        let mut component = mover(mover_id);
        component.block_policy = policy;
        component.crush_damage = 10.0;
        registry.set_component(mover_entity, component).unwrap();
        let item = add_enemy(&mut registry, Some(47.0));
        let agent = registry
            .get_component::<AgentComponent>(item)
            .unwrap()
            .clone();
        let mut inventory = Inventory::default();
        inventory.wieldables[9] = Some(item);
        registry.set_component(holder, inventory).unwrap();
        let collider = swept_wall(mover_id);
        let poses = moving_contact_pose(mover_id);
        let mut state = MoverBlockingState::default();
        let mut events = Vec::new();
        run_mover_blocking_pass(
            &mut registry,
            &CollisionWorld::new(),
            std::slice::from_ref(&collider),
            &poses,
            &mut state,
            0.05,
            &mut events,
            &mut |_| panic!("held item must not dispatch crush policy"),
        );
        assert!(events.is_empty());
        assert_eq!(
            registry
                .get_component::<HealthComponent>(item)
                .unwrap()
                .current,
            47.0
        );
        assert_eq!(
            registry.get_component::<AgentComponent>(item).unwrap(),
            &agent
        );
        assert!(
            !registry
                .get_component::<KinematicMoverComponent>(mover_entity)
                .unwrap()
                .blocked
        );
        assert_eq!(
            registry
                .get_component::<KinematicMoverComponent>(mover_entity)
                .unwrap()
                .direction_sign,
            1
        );
        registry
            .set_component(holder, Inventory::default())
            .unwrap();
        run_mover_blocking_pass(
            &mut registry,
            &CollisionWorld::new(),
            std::slice::from_ref(&collider),
            &poses,
            &mut state,
            0.05,
            &mut events,
            &mut |_| {},
        );
        assert_eq!(events.len(), 1, "released item resumes mover participation");
        if policy == BlockPolicy::Crush {
            assert_eq!(
                registry
                    .get_component::<HealthComponent>(item)
                    .unwrap()
                    .current,
                37.0
            );
        }
    }
}

#[test]
fn held_world_participation_rechecks_queued_crush_victims_after_callback() {
    use postretro_entities::components::inventory::Inventory;
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let holder = registry.spawn(Transform::default());
    let mover_entity = registry.spawn(Transform::default());
    let mut component = mover(mover_id);
    component.block_policy = BlockPolicy::Crush;
    component.crush_damage = 10.0;
    registry.set_component(mover_entity, component).unwrap();
    let first = add_enemy(&mut registry, Some(47.0));
    let second = add_enemy(&mut registry, Some(47.0));
    let collider = swept_wall(mover_id);
    let poses = moving_contact_pose(mover_id);
    let mut callbacks = 0;
    let mut events = Vec::new();
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&collider),
        &poses,
        &mut MoverBlockingState::default(),
        0.05,
        &mut events,
        &mut |registry| {
            callbacks += 1;
            // Contact iteration order is deliberately irrelevant to this proof.
            let mut inventory = Inventory::default();
            inventory.wieldables[8] = Some(first);
            inventory.wieldables[9] = Some(second);
            registry.set_component(holder, inventory).unwrap();
        },
    );
    assert_eq!(callbacks, 1);
    assert_eq!(events, vec![(MoverEventKind::Crushed, mover_id)]);
    let healths: Vec<_> = [first, second]
        .into_iter()
        .map(|id| {
            registry
                .get_component::<HealthComponent>(id)
                .unwrap()
                .current
        })
        .collect();
    assert_eq!(healths.iter().sum::<f32>(), 84.0);
}
