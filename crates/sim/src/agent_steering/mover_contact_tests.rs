// Agents against movers through the steering tick: doors stop them, lifts carry
// them, displacing movers push them, and steering neither fights the substrate
// nor reads a carried rider as stuck.
// See: context/lib/movement.md §6

use glam::{Quat, Vec3};
use postretro_collision::EnvironmentView;
use postretro_entities::Transform;
use postretro_foundation::GroundRef;

use super::*;
use crate::collision::moving::{CombinedCollisionWorld, MoverCollider, MoverPose, MoverPoseSource};
use crate::collision::{CollisionWorld, SKIN_DISTANCE};

const DT: f32 = 1.0 / 60.0;
const ENV: EnvironmentView<'static> = EnvironmentView::level_only(-20.0);
const MOVER: u32 = 3;
const RADIUS: f32 = 0.35;
const HEIGHT: f32 = 1.8;
const SPEED: f32 = 4.0;

/// How close a body's displacement stays to its mover's carry.
const CARRY_TOLERANCE: f32 = 1.0e-4;

/// Capsule-centre height of an agent resting on a surface at `floor_y`.
fn rest_y(floor_y: f32) -> f32 {
    floor_y + HEIGHT / 2.0 + SKIN_DISTANCE
}

fn floor_world() -> CollisionWorld {
    let span = 60.0;
    CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(-span, 0.0, -span),
            Vec3::new(span, 0.0, -span),
            Vec3::new(span, 0.0, span),
            Vec3::new(-span, 0.0, span),
        ],
        vec![[0, 2, 1], [0, 3, 2]],
    )
}

/// A solid mover-local box, wound outward.
fn solid_box(min: Vec3, max: Vec3) -> MoverCollider {
    let at = |x: bool, y: bool, z: bool| {
        Vec3::new(
            if x { max.x } else { min.x },
            if y { max.y } else { min.y },
            if z { max.z } else { min.z },
        )
    };
    let faces = [
        (
            [
                at(false, false, false),
                at(false, true, false),
                at(false, true, true),
                at(false, false, true),
            ],
            Vec3::NEG_X,
        ),
        (
            [
                at(true, false, false),
                at(true, true, false),
                at(true, true, true),
                at(true, false, true),
            ],
            Vec3::X,
        ),
        (
            [
                at(false, false, false),
                at(true, false, false),
                at(true, false, true),
                at(false, false, true),
            ],
            Vec3::NEG_Y,
        ),
        (
            [
                at(false, true, false),
                at(true, true, false),
                at(true, true, true),
                at(false, true, true),
            ],
            Vec3::Y,
        ),
        (
            [
                at(false, false, false),
                at(true, false, false),
                at(true, true, false),
                at(false, true, false),
            ],
            Vec3::NEG_Z,
        ),
        (
            [
                at(false, false, true),
                at(true, false, true),
                at(true, true, true),
                at(false, true, true),
            ],
            Vec3::Z,
        ),
    ];
    let mut points = Vec::new();
    let mut triangles = Vec::new();
    for (corners, normal) in faces {
        let base = points.len() as u32;
        points.extend(corners);
        let face = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
        if face.dot(normal) >= 0.0 {
            triangles.push([base, base + 1, base + 2]);
            triangles.push([base, base + 2, base + 3]);
        } else {
            triangles.push([base, base + 2, base + 1]);
            triangles.push([base, base + 3, base + 2]);
        }
    }
    MoverCollider::from_local_triangles(MOVER, &points, &triangles).unwrap()
}

/// One mover's pose, advanced tick by tick.
struct MoverTrack {
    pose: MoverPose,
}

impl MoverTrack {
    fn at(position: Vec3) -> Self {
        Self {
            pose: MoverPose {
                transform: Transform {
                    position,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::ZERO,
                tick_rotation_delta: Quat::IDENTITY,
                carry_yaw: false,
                tick_dt: DT,
            },
        }
    }

    fn advance(&mut self, velocity: Vec3) {
        self.pose.linear_velocity = velocity;
        self.pose.tick_delta = velocity * DT;
        self.pose.transform.position += self.pose.tick_delta;
    }
}

impl MoverPoseSource for MoverTrack {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        (mover_id == MOVER).then_some(self.pose)
    }
}

/// An agent at rest at `position`, already following a straight path to `goal`
/// when one is given.
fn spawn(registry: &mut EntityRegistry, position: Vec3, goal: Option<Vec3>) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    let mut agent = AgentComponent::new(RADIUS, HEIGHT, 0.4, SPEED);
    if let Some(goal) = goal {
        agent.destination = Some(goal);
        agent.planned_destination = Some(goal);
        agent.path = vec![goal];
        agent.replan_cooldown_ticks = u32::MAX;
    }
    registry.set_component(id, agent).unwrap();
    id
}

fn position(registry: &EntityRegistry, id: EntityId) -> Vec3 {
    registry.get_component::<Transform>(id).unwrap().position
}

fn agent(registry: &EntityRegistry, id: EntityId) -> &AgentComponent {
    registry.get_component::<AgentComponent>(id).unwrap()
}

fn step(
    registry: &mut EntityRegistry,
    world: &CollisionWorld,
    movers: &[MoverCollider],
    track: &MoverTrack,
) {
    let collision = CombinedCollisionWorld::new(world, movers, track);
    tick(registry, &collision, None, ENV, DT);
}

#[test]
fn agent_stops_at_closed_mover_and_passes_once_open() {
    let world = floor_world();
    // A door 0.5 m thick across the agent's way, its near face at x = 5.
    let movers = [solid_box(
        Vec3::new(0.0, 0.0, -20.0),
        Vec3::new(0.5, 3.0, 20.0),
    )];
    let mut door = MoverTrack::at(Vec3::new(5.0, 0.0, 0.0));
    let mut registry = EntityRegistry::new();
    let goal = Vec3::new(12.0, rest_y(0.0), 0.0);
    let id = spawn(&mut registry, Vec3::new(0.0, rest_y(0.0), 0.0), Some(goal));

    for tick_index in 0..180 {
        door.advance(Vec3::ZERO);
        step(&mut registry, &world, &movers, &door);
        let at = position(&registry, id);
        assert!(
            at.x + RADIUS <= 5.0 + 1.0e-3,
            "tick {tick_index}: the agent passed the closed door, at {at:?}"
        );
        assert!(
            (at.y - rest_y(0.0)).abs() < 1.0e-3,
            "tick {tick_index}: the agent climbed or bounced off the door, at {at:?}"
        );
    }
    assert!(
        position(&registry, id).x + RADIUS > 5.0 - 0.1,
        "the agent reached the door"
    );

    // The door lifts away; the agent walks through.
    for _ in 0..30 {
        door.advance(Vec3::Y * 8.0);
        step(&mut registry, &world, &movers, &door);
    }
    for _ in 0..180 {
        door.advance(Vec3::ZERO);
        step(&mut registry, &world, &movers, &door);
    }
    assert!(
        position(&registry, id).x > 6.0,
        "the agent passed the open door, at {:?}",
        position(&registry, id)
    );
}

#[test]
fn displace_mover_pushes_unpinned_agent_clear() {
    let world = floor_world();
    // A sweeper's near face at local x = 0.5, moving +X into an idle agent.
    let movers = [solid_box(
        Vec3::new(0.0, 0.0, -20.0),
        Vec3::new(0.5, 3.0, 20.0),
    )];
    let mut sweeper = MoverTrack::at(Vec3::new(-2.0, 0.0, 0.0));
    let mut registry = EntityRegistry::new();
    let id = spawn(&mut registry, Vec3::new(0.0, rest_y(0.0), 0.0), None);

    for tick_index in 0..120 {
        sweeper.advance(Vec3::X * 2.0);
        step(&mut registry, &world, &movers, &sweeper);
        let face = sweeper.pose.transform.position.x + 0.5;
        let at = position(&registry, id);
        assert!(
            at.x - RADIUS >= face - 1.0e-3,
            "tick {tick_index}: the sweeper embedded the agent: face {face}, agent {at:?}"
        );
    }
    assert!(
        position(&registry, id).x > 1.0,
        "the sweeper pushed the agent along"
    );
}

/// A lift top at y = 0.5, wide enough to walk on, that starts, reverses and
/// stops under a rider. Returns the lift velocity on `tick`.
fn lift_course(tick: u32) -> Vec3 {
    match tick {
        20..80 => Vec3::Y * 2.0,
        80..110 => Vec3::NEG_Y * 2.0,
        _ => Vec3::ZERO,
    }
}

fn ride(goal: Option<Vec3>) {
    let world = floor_world();
    let movers = [solid_box(
        Vec3::new(-20.0, -0.5, -8.0),
        Vec3::new(20.0, 0.0, 8.0),
    )];
    let mut lift = MoverTrack::at(Vec3::new(0.0, 0.5, 0.0));
    let mut registry = EntityRegistry::new();
    let id = spawn(&mut registry, Vec3::new(0.0, rest_y(0.5), 0.0), goal);

    let mut before = position(&registry, id);
    for tick_index in 0..140 {
        let velocity = lift_course(tick_index);
        lift.advance(velocity);
        step(&mut registry, &world, &movers, &lift);
        let at = position(&registry, id);
        if tick_index > 0 {
            assert_eq!(
                agent(&registry, id).ground,
                GroundRef::Mover(MOVER),
                "tick {tick_index}: mover ground"
            );
            let drift = (at.y - before.y) - velocity.y * DT;
            assert!(
                drift.abs() < CARRY_TOLERANCE,
                "tick {tick_index}: rider drifted {drift} from the carry"
            );
        }
        before = at;
    }
}

#[test]
fn an_idle_agent_rides_a_lift_through_start_reverse_and_stop() {
    ride(None);
}

#[test]
fn a_walking_agent_rides_a_lift_through_start_reverse_and_stop() {
    ride(Some(Vec3::new(30.0, rest_y(0.5), 0.0)));
}

#[test]
fn a_rider_walking_against_its_mover_is_not_stuck() {
    let world = floor_world();
    let movers = [solid_box(
        Vec3::new(-30.0, -0.5, -8.0),
        Vec3::new(30.0, 0.0, 8.0),
    )];
    let mut lift = MoverTrack::at(Vec3::new(0.0, 0.5, 0.0));
    let mut registry = EntityRegistry::new();
    let start = Vec3::new(0.0, rest_y(0.5), 0.0);
    let id = spawn(
        &mut registry,
        start,
        Some(Vec3::new(50.0, rest_y(0.5), 0.0)),
    );

    // The platform runs against the agent at its own speed, so it treads in
    // place in world space: no goal progress, and nothing wrong.
    for tick_index in 0..120 {
        lift.advance(Vec3::NEG_X * SPEED);
        step(&mut registry, &world, &movers, &lift);
        let rider = agent(&registry, id);
        assert_eq!(rider.stuck_ticks, 0, "tick {tick_index}: stuck count");
        assert_eq!(
            rider.unstick_window_remaining, 0,
            "tick {tick_index}: recovery window"
        );
    }
}

/// A platform 8 m wide under an agent at the given height above it, descending
/// faster than ground-stick reaches.
#[test]
fn an_agent_above_a_descender_beyond_ground_stick_falls_under_gravity() {
    let world = floor_world();
    let movers = [solid_box(
        Vec3::new(-8.0, -0.5, -8.0),
        Vec3::new(8.0, 0.0, 8.0),
    )];
    let mut lift = MoverTrack::at(Vec3::new(0.0, 6.0, 0.0));
    let mut registry = EntityRegistry::new();
    let id = spawn(&mut registry, Vec3::new(0.0, rest_y(6.0) + 1.5, 0.0), None);

    let start = position(&registry, id).y;
    for tick_index in 1..=6 {
        lift.advance(Vec3::NEG_Y * 2.0);
        step(&mut registry, &world, &movers, &lift);
        assert_eq!(
            agent(&registry, id).ground,
            GroundRef::Airborne,
            "tick {tick_index}: no floor in reach"
        );
    }
    let fallen = start - position(&registry, id).y;
    let free_fall = 0.5 * 20.0 * (6.0 * DT) * (6.0 * DT);
    assert!(
        (fallen - free_fall).abs() < 0.05,
        "fell {fallen} m, free fall is {free_fall} m"
    );
}

#[test]
fn stepping_off_a_moving_mover_adds_release_velocity_that_decays_with_knockback_drag() {
    let world = floor_world();
    // A 2 m platform 0.2 m over the floor, running +X, its rider walking off.
    let movers = [solid_box(
        Vec3::new(-1.0, -0.2, -8.0),
        Vec3::new(1.0, 0.0, 8.0),
    )];
    let mut lift = MoverTrack::at(Vec3::new(0.0, 0.2, 0.0));
    let mut registry = EntityRegistry::new();
    let id = spawn(
        &mut registry,
        Vec3::new(0.0, rest_y(0.2), 0.0),
        Some(Vec3::new(0.0, rest_y(0.0), 40.0)),
    );
    let drive = Vec3::X * 3.0;

    let mut released = None;
    for tick_index in 0..200 {
        lift.advance(drive);
        step(&mut registry, &world, &movers, &lift);
        let rider = agent(&registry, id);
        if rider.knockback_velocity != Vec3::ZERO {
            released = Some((tick_index, rider.knockback_velocity));
            break;
        }
    }
    let (_, release) = released.expect("the agent left the mover");
    assert!(
        (release.x - drive.x).abs() < 1.0e-4,
        "the mover's velocity entered the knockback layer: {release:?}"
    );

    // Steering does not wipe it; drag does.
    let mut previous = release.x;
    for _ in 0..30 {
        lift.advance(drive);
        step(&mut registry, &world, &movers, &lift);
        let now = agent(&registry, id).knockback_velocity.x;
        assert!(now <= previous, "knockback decays, {now} after {previous}");
        previous = now;
    }
    assert!(
        previous < release.x,
        "drag took some of it: {previous} of {}",
        release.x
    );
}

fn corpse_of(registry: &mut EntityRegistry, id: EntityId) {
    registry
        .set_component(
            id,
            postretro_entities::components::health::HealthComponent {
                max: 100.0,
                current: 0.0,
                hitbox: None,
                death_handled: false,
                pending_kill_credit: None,
                zone_multipliers: Default::default(),
                contributor_ledger: Default::default(),
            },
        )
        .unwrap();
}

#[test]
fn a_corpse_rides_a_lift_and_takes_no_steering() {
    let world = floor_world();
    let movers = [solid_box(
        Vec3::new(-8.0, -0.5, -8.0),
        Vec3::new(8.0, 0.0, 8.0),
    )];
    let mut lift = MoverTrack::at(Vec3::new(0.0, 0.5, 0.0));
    let mut registry = EntityRegistry::new();
    let id = spawn(
        &mut registry,
        Vec3::new(1.0, rest_y(0.5), 0.0),
        Some(Vec3::new(30.0, rest_y(0.5), 0.0)),
    );
    corpse_of(&mut registry, id);

    let mut before = position(&registry, id);
    for tick_index in 0..120 {
        let velocity = lift_course(tick_index);
        lift.advance(velocity);
        step(&mut registry, &world, &movers, &lift);
        let at = position(&registry, id);
        assert_eq!(at.x, 1.0, "tick {tick_index}: no steering");
        if tick_index > 0 {
            assert_eq!(agent(&registry, id).ground, GroundRef::Mover(MOVER));
            assert!(((at.y - before.y) - velocity.y * DT).abs() < CARRY_TOLERANCE);
        }
        before = at;
    }
}

#[test]
fn a_corpse_falls_and_a_killing_blow_starts_no_slide() {
    let world = floor_world();
    let mut registry = EntityRegistry::new();
    let id = spawn(&mut registry, Vec3::new(0.0, rest_y(0.0) + 3.0, 0.0), None);
    corpse_of(&mut registry, id);
    let mut body = agent(&registry, id).clone();
    body.knockback_velocity = Vec3::new(5.0, 2.0, 0.0);
    body.velocity = body.knockback_velocity;
    registry.set_component(id, body).unwrap();

    for _ in 0..120 {
        tick(&mut registry, &world, None, ENV, DT);
    }
    let at = position(&registry, id);
    assert_eq!(at.x, 0.0, "no slide");
    assert!(
        (at.y - rest_y(0.0)).abs() < 2.0e-3,
        "the corpse lay down on the floor, at {at:?}"
    );
    assert_eq!(agent(&registry, id).ground, GroundRef::World);
    assert_eq!(agent(&registry, id).knockback_velocity, Vec3::ZERO);
}

#[test]
fn a_corpse_leaves_a_moving_mover_with_no_release_velocity() {
    let world = floor_world();
    let movers = [solid_box(
        Vec3::new(-8.0, -0.5, -8.0),
        Vec3::new(8.0, 0.0, 8.0),
    )];
    let mut lift = MoverTrack::at(Vec3::new(0.0, 0.5, 0.0));
    let mut registry = EntityRegistry::new();
    let id = spawn(&mut registry, Vec3::new(0.0, rest_y(0.5), 0.0), None);
    corpse_of(&mut registry, id);
    lift.advance(Vec3::ZERO);
    step(&mut registry, &world, &movers, &lift);
    assert_eq!(agent(&registry, id).ground, GroundRef::Mover(MOVER));

    // The platform leaves sideways from under it at speed.
    for _ in 0..60 {
        lift.advance(Vec3::X * 12.0);
        step(&mut registry, &world, &movers, &lift);
    }
    let body = agent(&registry, id);
    assert_eq!(body.knockback_velocity, Vec3::ZERO);
    assert_eq!(body.velocity.x, 0.0);
}

/// The same lift ride and walk twice lands on the same bits.
#[test]
fn an_agent_on_a_lift_replays_identically() {
    fn run() -> Vec<(u32, u32, u32, GroundRef)> {
        let world = floor_world();
        let movers = [solid_box(
            Vec3::new(-8.0, -0.5, -8.0),
            Vec3::new(8.0, 0.0, 8.0),
        )];
        let mut lift = MoverTrack::at(Vec3::new(0.0, 0.5, 0.0));
        let mut registry = EntityRegistry::new();
        let id = spawn(
            &mut registry,
            Vec3::new(0.0, rest_y(0.5), 0.0),
            Some(Vec3::new(30.0, rest_y(0.0), 3.0)),
        );
        (0..240)
            .map(|tick_index| {
                lift.advance(lift_course(tick_index));
                step(&mut registry, &world, &movers, &lift);
                let at = position(&registry, id);
                (
                    at.x.to_bits(),
                    at.y.to_bits(),
                    at.z.to_bits(),
                    agent(&registry, id).ground,
                )
            })
            .collect()
    }
    assert_eq!(run(), run());
}
