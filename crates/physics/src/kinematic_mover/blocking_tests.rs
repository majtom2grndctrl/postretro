// Host blocking-pass tests: each policy's reactions to player and agent contacts.
// Pinned-body yields are proved in tick order in pin_tests.rs.
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
            friction: GroundParams::DEFAULT_FRICTION,
            stop_speed: GroundParams::DEFAULT_STOP_SPEED,
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

fn set_enemy_ground(registry: &mut EntityRegistry, enemy: EntityId, ground: GroundRef) {
    let mut agent = registry
        .get_component::<AgentComponent>(enemy)
        .expect("agent remains attached")
        .clone();
    agent.ground = ground;
    registry.set_component(enemy, agent).expect("agent updates");
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
        &blocking_static_wall(),
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
        &blocking_static_wall(),
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
        &blocking_static_wall(),
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
fn crush_policy_damages_pinned_enemy_and_continues_after_death_latch() {
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

    run_pass(&mut registry); // First pinned hit lands immediately.
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
fn displace_policy_ignores_unpinned_enemy_contacts() {
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
    let static_world = blocking_static_wall();
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

    run_pass(&mut registry); // First pinned hit lands immediately.
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
        let world = if policy == BlockPolicy::Crush {
            blocking_static_wall()
        } else {
            CollisionWorld::new()
        };
        run_mover_blocking_pass(
            &mut registry,
            &world,
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
            &world,
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
        &blocking_static_wall(),
        std::slice::from_ref(&collider),
        &poses,
        &mut MoverBlockingState::default(),
        0.05,
        &mut events,
        &mut |registry| {
            callbacks += 1;
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

// A crush resolves its type name against the live vocabulary on each hit: a
// reload that reorders the types keeps it, and an unregistered name deals the
// default type.
#[test]
fn crush_deals_its_crush_damage_type() {
    use postretro_foundation::defense::{DamageTypeDeclaration, DefenseVocabulary, PoolMultiplier};
    let declare = |name: &str, scale: f32| DamageTypeDeclaration {
        name: name.to_string(),
        matrix: std::collections::BTreeMap::from([(
            "health".to_string(),
            PoolMultiplier::Scale(scale),
        )]),
    };
    let vocabulary = |damage_types| {
        std::sync::Arc::new(DefenseVocabulary {
            layers: Vec::new(),
            damage_types,
            default_damage_type: None,
        })
    };
    let mover_id = 43;
    let mut registry = EntityRegistry::new();
    registry.set_defense_vocabulary(vocabulary(vec![declare("heavy", 2.0)]));
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.completed = true;
    mover.crush_damage = 10.0;
    mover.crush_damage_type = Some("heavy".to_string());
    registry.set_component(mover_entity, mover).unwrap();
    let player = add_player(&mut registry, Some(100.0));
    set_player_ground(&mut registry, player, GroundRef::Mover(mover_id));
    let crush = |registry: &mut EntityRegistry| {
        run_mover_blocking_pass(
            registry,
            &blocking_static_wall(),
            std::slice::from_ref(&swept_wall(mover_id)),
            &moving_contact_pose(mover_id),
            &mut MoverBlockingState::default(),
            0.1,
            &mut Vec::new(),
            &mut |_| {},
        );
        registry
            .get_component::<HealthComponent>(player)
            .unwrap()
            .current
    };

    assert_eq!(crush(&mut registry), 80.0, "a heavy crush is ×2 on health");
    registry.set_defense_vocabulary(vocabulary(vec![
        declare("light", 1.0),
        declare("heavy", 2.0),
    ]));
    assert_eq!(crush(&mut registry), 60.0, "the name survives a reorder");
    let mut renamed = registry
        .get_component::<KinematicMoverComponent>(mover_entity)
        .unwrap()
        .clone();
    renamed.crush_damage_type = Some("missing".to_string());
    registry.set_component(mover_entity, renamed).unwrap();
    assert_eq!(
        crush(&mut registry),
        50.0,
        "an unregistered name deals the default type"
    );
}

fn is_blocked(registry: &EntityRegistry, mover: EntityId) -> bool {
    registry
        .get_component::<KinematicMoverComponent>(mover)
        .expect("mover remains live")
        .blocked
}

fn health_of(registry: &EntityRegistry, entity: EntityId) -> f32 {
    registry
        .get_component::<HealthComponent>(entity)
        .expect("health remains attached")
        .current
}

/// One pass at `tick_dt` against an empty world, the mover moving into contact.
fn contact_pass(
    registry: &mut EntityRegistry,
    mover_id: u32,
    state: &mut MoverBlockingState,
    tick_dt: f32,
    events: &mut Vec<(MoverEventKind, u32)>,
) {
    run_mover_blocking_pass(
        registry,
        &CollisionWorld::new(),
        std::slice::from_ref(&swept_wall(mover_id)),
        &moving_contact_pose(mover_id),
        state,
        tick_dt,
        events,
        &mut |_| {},
    );
}

// --- Agents as riders ----------------------------------------------------------------------------

#[test]
fn grounded_agent_does_not_stop_or_reverse_its_mover() {
    for policy in [BlockPolicy::Stop, BlockPolicy::Reverse] {
        let mover_id = 42;
        let mut registry = EntityRegistry::new();
        let mover_entity = registry.spawn(Transform::default());
        let mut mover = mover(mover_id);
        mover.block_policy = policy;
        registry.set_component(mover_entity, mover).unwrap();
        let enemy = add_enemy(&mut registry, None);
        set_enemy_ground(&mut registry, enemy, GroundRef::Mover(mover_id));

        let mut state = MoverBlockingState::default();
        let mut events = Vec::new();
        for _ in 0..2 {
            contact_pass(&mut registry, mover_id, &mut state, 0.1, &mut events);
        }

        let mover = registry
            .get_component::<KinematicMoverComponent>(mover_entity)
            .unwrap();
        assert!(!mover.blocked, "{policy:?}: its own rider holds nothing");
        assert_eq!(mover.direction_sign, 1, "{policy:?}: nor reverses it");
        assert!(events.is_empty());
    }
}

#[test]
fn airborne_rider_grace_keeps_a_departing_agent_from_stopping_then_expires() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Stop;
    registry.set_component(mover_entity, mover).unwrap();
    let enemy = add_enemy(&mut registry, None);
    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();

    set_enemy_ground(&mut registry, enemy, GroundRef::Mover(mover_id));
    contact_pass(&mut registry, mover_id, &mut state, 0.1, &mut events);

    set_enemy_ground(&mut registry, enemy, GroundRef::Airborne);
    contact_pass(&mut registry, mover_id, &mut state, 0.1, &mut events);
    assert!(events.is_empty(), "a fresh grace window skips the contact");
    assert!(!is_blocked(&registry, mover_entity));

    contact_pass(&mut registry, mover_id, &mut state, 0.051, &mut events);
    assert!(
        is_blocked(&registry, mover_entity),
        "the bounded airborne exemption expires after RIDER_GRACE_MS"
    );
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);
}

#[test]
fn airborne_rider_grace_keeps_a_departing_agent_from_reversing() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Reverse;
    registry.set_component(mover_entity, mover).unwrap();
    let enemy = add_enemy(&mut registry, None);
    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();

    set_enemy_ground(&mut registry, enemy, GroundRef::Mover(mover_id));
    contact_pass(&mut registry, mover_id, &mut state, 0.1, &mut events);
    set_enemy_ground(&mut registry, enemy, GroundRef::Airborne);
    contact_pass(&mut registry, mover_id, &mut state, 0.1, &mut events);

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
fn agent_riding_another_mover_or_past_its_grace_still_stops_and_reverses_this_one() {
    for policy in [BlockPolicy::Stop, BlockPolicy::Reverse] {
        for ground in [GroundRef::Mover(7), GroundRef::World, GroundRef::Airborne] {
            let mover_id = 42;
            let mut registry = EntityRegistry::new();
            let mover_entity = registry.spawn(Transform::default());
            let mut mover = mover(mover_id);
            mover.block_policy = policy;
            registry.set_component(mover_entity, mover).unwrap();
            let enemy = add_enemy(&mut registry, None);
            set_enemy_ground(&mut registry, enemy, ground);

            let mut events = Vec::new();
            contact_pass(
                &mut registry,
                mover_id,
                &mut MoverBlockingState::default(),
                0.1,
                &mut events,
            );

            assert_eq!(
                events,
                vec![(MoverEventKind::Blocked, mover_id)],
                "{policy:?} reacts to an agent on {ground:?}"
            );
        }
    }
}

// --- `crush` and agents --------------------------------------------------------------------------

#[test]
fn crusher_overlapping_an_agent_in_open_air_deals_no_damage() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 10.0;
    registry.set_component(mover_entity, mover).unwrap();
    let walker = add_enemy(&mut registry, Some(100.0));
    let rider = add_enemy(&mut registry, Some(100.0));
    set_enemy_ground(&mut registry, rider, GroundRef::Mover(mover_id));

    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();
    for _ in 0..4 {
        contact_pass(&mut registry, mover_id, &mut state, 0.1, &mut events);
    }

    assert_eq!(health_of(&registry, walker), 100.0);
    assert_eq!(health_of(&registry, rider), 100.0);
    assert!(events.is_empty());
    assert!(registry.take_impact_dispatches().is_empty());
}

#[test]
fn crusher_pressing_an_agent_against_another_mover_damages_it() {
    let mover_id = 42;
    let other_id = 43;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut crusher = mover(mover_id);
    crusher.block_policy = BlockPolicy::Crush;
    crusher.crush_damage = 10.0;
    registry.set_component(mover_entity, crusher).unwrap();
    let other_entity = registry.spawn(Transform::default());
    let mut parked = mover(other_id);
    parked.started = false;
    registry.set_component(other_entity, parked).unwrap();
    let enemy = add_enemy(&mut registry, Some(100.0));

    // The same plane as the static wall fixture, as a parked mover.
    let parked_wall = MoverCollider::from_local_triangles(
        other_id,
        &[
            Vec3::new(0.1, 0.0, -1.0),
            Vec3::new(0.1, 2.0, -1.0),
            Vec3::new(0.1, 2.0, 1.0),
            Vec3::new(0.1, 0.0, 1.0),
        ],
        &[[0, 1, 2], [0, 2, 3]],
    )
    .expect("wall fixture is valid");
    let colliders = [swept_wall(mover_id), parked_wall];
    let poses = TwoMoverPoses {
        first: moving_contact_pose(mover_id),
        second: stationary_contact_pose(other_id),
    };
    let mut events = Vec::new();
    run_mover_blocking_pass(
        &mut registry,
        &CollisionWorld::new(),
        &colliders,
        &poses,
        &mut MoverBlockingState::default(),
        0.1,
        &mut events,
        &mut |_| {},
    );

    assert_eq!(health_of(&registry, enemy), 90.0);
    assert_eq!(events, vec![(MoverEventKind::Crushed, mover_id)]);
}

/// A closed box around the origin's upper half, large enough to hold a whole
/// agent standing at `Vec3::Y` clear of every face.
fn engulfing_box(mover_id: u32) -> MoverCollider {
    let min = Vec3::new(-2.0, -2.0, -2.0);
    let max = Vec3::new(2.0, 4.0, 2.0);
    let corners = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];
    MoverCollider::from_local_triangles(
        mover_id,
        &corners,
        &[
            [0, 1, 2],
            [0, 2, 3],
            [4, 6, 5],
            [4, 7, 6],
            [0, 4, 5],
            [0, 5, 1],
            [1, 5, 6],
            [1, 6, 2],
            [2, 6, 7],
            [2, 7, 3],
            [3, 7, 4],
            [3, 4, 0],
        ],
    )
    .expect("box fixture is valid")
}

// A hull that closed around an agent in one tick reports no contact at all, and
// the agent's own displacement has nothing to push it out by.
#[test]
fn crusher_that_engulfs_an_agent_damages_it_with_no_contact_to_read() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 10.0;
    mover.crush_interval_ms = 100.0;
    registry.set_component(mover_entity, mover).unwrap();
    let enemy = add_enemy(&mut registry, Some(100.0));
    let player = add_player(&mut registry, Some(100.0));

    let collider = engulfing_box(mover_id);
    let poses = stationary_contact_pose(mover_id);
    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();
    for _ in 0..3 {
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
    }

    assert_eq!(
        health_of(&registry, enemy),
        80.0,
        "hit at once, then at the cadence"
    );
    assert_eq!(
        health_of(&registry, player),
        100.0,
        "the player path has no engulfment test"
    );
    assert_eq!(events.len(), 2);
    assert!(!is_blocked(&registry, mover_entity));
}

#[test]
fn corpse_pinned_by_a_crusher_takes_hits_and_raises_impact_facts_at_the_cadence() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 10.0;
    mover.crush_interval_ms = 100.0;
    registry.set_component(mover_entity, mover).unwrap();
    let corpse = add_enemy(&mut registry, Some(0.0));

    let collider = swept_wall(mover_id);
    let world = blocking_static_wall();
    let poses = moving_contact_pose(mover_id);
    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();
    let mut facts = Vec::new();
    for _ in 0..4 {
        run_mover_blocking_pass(
            &mut registry,
            &world,
            std::slice::from_ref(&collider),
            &poses,
            &mut state,
            0.05,
            &mut events,
            &mut |registry| facts.extend(registry.take_impact_dispatches()),
        );
    }

    assert_eq!(facts.len(), 2, "one fact per cadence, not per tick");
    assert!(facts.iter().all(|fact| fact.target == corpse));
    assert!(
        facts.iter().all(|fact| fact.health_before <= 0.0),
        "an impact policy keys on health left at 0"
    );
    assert_eq!(events.len(), 2);
    assert!(!is_blocked(&registry, mover_entity));
}

#[test]
fn agent_with_a_queued_despawn_is_still_crushed_as_a_corpse() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 10.0;
    registry.set_component(mover_entity, mover).unwrap();
    let enemy = add_enemy(&mut registry, Some(100.0));
    postretro_sim::impact_effects::despawn(&mut registry, enemy, Some(500.0));

    let mut events = Vec::new();
    run_mover_blocking_pass(
        &mut registry,
        &blocking_static_wall(),
        std::slice::from_ref(&swept_wall(mover_id)),
        &moving_contact_pose(mover_id),
        &mut MoverBlockingState::default(),
        0.1,
        &mut events,
        &mut |_| {},
    );

    assert_eq!(health_of(&registry, enemy), 90.0);
    assert_eq!(events, vec![(MoverEventKind::Crushed, mover_id)]);
}

#[test]
fn corpse_contact_never_stops_or_reverses_a_mover() {
    for policy in [BlockPolicy::Stop, BlockPolicy::Reverse] {
        for despawning in [false, true] {
            let mover_id = 42;
            let mut registry = EntityRegistry::new();
            let mover_entity = registry.spawn(Transform::default());
            let mut mover = mover(mover_id);
            mover.block_policy = policy;
            registry.set_component(mover_entity, mover).unwrap();
            if despawning {
                let enemy = add_enemy(&mut registry, Some(100.0));
                postretro_sim::impact_effects::despawn(&mut registry, enemy, Some(500.0));
            } else {
                add_enemy(&mut registry, Some(0.0));
            }

            let mut events = Vec::new();
            contact_pass(
                &mut registry,
                mover_id,
                &mut MoverBlockingState::default(),
                0.1,
                &mut events,
            );

            let mover = registry
                .get_component::<KinematicMoverComponent>(mover_entity)
                .unwrap();
            assert!(!mover.blocked && mover.direction_sign == 1);
            assert!(events.is_empty());
        }
    }
}

// Health resisting the crush type at 0 is the whole of crush immunity: the hit
// lands for 0 points, its facts still raise, and nothing makes the crusher yield.
#[test]
fn crush_immune_agent_takes_no_points_and_never_holds_the_crusher() {
    use postretro_foundation::defense::{DamageTypeDeclaration, DefenseVocabulary, PoolMultiplier};
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    registry.set_defense_vocabulary(std::sync::Arc::new(DefenseVocabulary {
        layers: Vec::new(),
        damage_types: vec![DamageTypeDeclaration {
            name: "crush".to_string(),
            matrix: std::collections::BTreeMap::from([(
                "health".to_string(),
                PoolMultiplier::Scale(0.0),
            )]),
        }],
        default_damage_type: None,
    }));
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Crush;
    mover.crush_damage = 10.0;
    mover.crush_interval_ms = 100.0;
    mover.crush_damage_type = Some("crush".to_string());
    registry.set_component(mover_entity, mover).unwrap();
    let enemy = add_enemy(&mut registry, Some(100.0));

    let collider = swept_wall(mover_id);
    let world = blocking_static_wall();
    let poses = moving_contact_pose(mover_id);
    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();
    let mut facts = Vec::new();
    for _ in 0..3 {
        run_mover_blocking_pass(
            &mut registry,
            &world,
            std::slice::from_ref(&collider),
            &poses,
            &mut state,
            0.05,
            &mut events,
            &mut |registry| facts.extend(registry.take_impact_dispatches()),
        );
    }

    assert_eq!(health_of(&registry, enemy), 100.0);
    assert_eq!(facts.len(), 2);
    assert!(facts.iter().all(|fact| fact.amount == 0.0));
    assert!(!is_blocked(&registry, mover_entity));
}

// --- Crush effect order --------------------------------------------------------------------------

// Regression: damage, events and impact callbacks ran in `HashSet` order, which
// a fresh random seed changes on every set.
#[test]
fn multi_victim_crush_runs_its_effects_in_one_order_across_runs() {
    fn run() -> (Vec<(EntityId, f32)>, Vec<(MoverEventKind, u32)>) {
        let (first_id, second_id) = (42, 43);
        let mut registry = EntityRegistry::new();
        for mover_id in [first_id, second_id] {
            let entity = registry.spawn(Transform::default());
            let mut mover = mover(mover_id);
            mover.block_policy = BlockPolicy::Crush;
            mover.crush_damage = mover_id as f32;
            registry.set_component(entity, mover).unwrap();
        }
        add_player(&mut registry, Some(100.0));
        for _ in 0..4 {
            add_enemy(&mut registry, Some(100.0));
        }
        add_enemy(&mut registry, Some(0.0));
        add_player(&mut registry, Some(100.0));

        let colliders = [swept_wall(first_id), swept_wall(second_id)];
        let poses = TwoMoverPoses {
            first: moving_contact_pose(first_id),
            second: moving_contact_pose(second_id),
        };
        let mut facts = Vec::new();
        let mut events = Vec::new();
        run_mover_blocking_pass(
            &mut registry,
            &blocking_static_wall(),
            &colliders,
            &poses,
            &mut MoverBlockingState::default(),
            0.1,
            &mut events,
            &mut |registry| {
                facts.extend(
                    registry
                        .take_impact_dispatches()
                        .into_iter()
                        .map(|fact| (fact.target, fact.raw)),
                );
            },
        );
        (facts, events)
    }

    let first = run();
    assert_eq!(first.0.len(), 14, "seven victims under two crushers");
    assert_eq!(first.1.len(), 14);
    for attempt in 1..16 {
        assert_eq!(run(), first, "run {attempt} kept the first run's order");
    }
}

// --- Corpse yields -------------------------------------------------------------------------------

#[test]
fn corpse_yield_stops_a_contact_reaction_and_drops_once_out_of_contact() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Stop;
    registry.set_component(mover_entity, mover).unwrap();
    let enemy = add_enemy(&mut registry, Some(100.0));
    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();
    let move_enemy_to = |registry: &mut EntityRegistry, x: f32| {
        let mut transform = *registry.get_component::<Transform>(enemy).unwrap();
        transform.position.x = x;
        registry.set_component(enemy, transform).unwrap();
    };

    state.yield_agent_as_corpse(enemy, mover_id);
    contact_pass(&mut registry, mover_id, &mut state, 0.1, &mut events);
    assert!(events.is_empty(), "the touching pair stops nothing");
    assert!(!is_blocked(&registry, mover_entity));
    assert!(state.agent_yields_as_corpse(enemy, mover_id));

    move_enemy_to(&mut registry, 30.0);
    contact_pass(&mut registry, mover_id, &mut state, 0.1, &mut events);
    assert!(!state.agent_yields_as_corpse(enemy, mover_id));

    move_enemy_to(&mut registry, 0.0);
    contact_pass(&mut registry, mover_id, &mut state, 0.1, &mut events);
    assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);
}

#[test]
fn corpse_yield_drops_with_its_agent_and_under_a_crusher() {
    let mover_id = 42;
    let mut registry = EntityRegistry::new();
    let mover_entity = registry.spawn(Transform::default());
    let mut mover = mover(mover_id);
    mover.block_policy = BlockPolicy::Stop;
    registry.set_component(mover_entity, mover).unwrap();
    let despawned = add_enemy(&mut registry, Some(100.0));
    let crushed = add_enemy(&mut registry, Some(100.0));
    let mut state = MoverBlockingState::default();
    let mut events = Vec::new();

    state.yield_agent_as_corpse(despawned, mover_id);
    registry.despawn(despawned).unwrap();
    contact_pass(&mut registry, mover_id, &mut state, 0.1, &mut events);
    assert!(!state.agent_yields_as_corpse(despawned, mover_id));

    let mut crusher = registry
        .get_component::<KinematicMoverComponent>(mover_entity)
        .unwrap()
        .clone();
    crusher.block_policy = BlockPolicy::Crush;
    registry.set_component(mover_entity, crusher).unwrap();
    state.yield_agent_as_corpse(crushed, mover_id);
    contact_pass(&mut registry, mover_id, &mut state, 0.1, &mut events);
    assert!(
        !state.agent_yields_as_corpse(crushed, mover_id),
        "nothing yields to a crusher as a corpse"
    );
}
