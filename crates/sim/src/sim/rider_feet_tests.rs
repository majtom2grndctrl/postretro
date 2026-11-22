use std::collections::HashMap;

use glam::{Quat, Vec3};
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::GroundRef;

use super::{PresentedMoverPoses, RiderFeet, update_rider_foot_probes};
use crate::collision::CollisionWorld;
use crate::collision::moving::MoverCollider;
use crate::kinematic_mover::{MoverTickState, MoverTickStateTable};
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::sim::determinism_tests::leg_probe_fixture;

const LIFT: u32 = 3;
const RISE_PER_TICK: f32 = 0.3;
const RIDER_HEIGHT: f32 = 1.0;
const BOUND: f32 = 1.0e-3;
const ALPHAS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

fn deck() -> MoverCollider {
    MoverCollider::from_local_triangles(
        LIFT,
        &[
            Vec3::new(-5.0, 0.0, -5.0),
            Vec3::new(5.0, 0.0, -5.0),
            Vec3::new(5.0, 0.0, 5.0),
            Vec3::new(-5.0, 0.0, 5.0),
        ],
        &[[0, 2, 1], [0, 3, 2]],
    )
    .unwrap()
}

fn at_height(y: f32) -> Transform {
    Transform {
        position: Vec3::new(0.0, y, 0.0),
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    }
}

/// A lift mid-rise, and a legged rider planted on it, both blending one tick
/// of rise; the tick table holds the lift's end-of-tick pose.
struct Ride {
    registry: EntityRegistry,
    rider: EntityId,
    store: HitZoneStore,
    table: MoverTickStateTable,
}

fn ride() -> Ride {
    let (mut registry, rider, store) = leg_probe_fixture(Vec3::new(0.0, RIDER_HEIGHT, 0.0), 0.0);
    let lift = registry.spawn(at_height(0.0));
    registry.snapshot_transforms();
    registry
        .set_component(lift, at_height(RISE_PER_TICK))
        .unwrap();
    registry
        .set_component(rider, at_height(RISE_PER_TICK + RIDER_HEIGHT))
        .unwrap();
    let mut table = MoverTickStateTable::default();
    table.publish(
        LIFT,
        MoverTickState {
            entity: lift,
            transform: at_height(RISE_PER_TICK),
            linear_velocity: Vec3::Y * RISE_PER_TICK * 60.0,
            tick_delta: Vec3::Y * RISE_PER_TICK,
            angular_velocity: Vec3::ZERO,
            tick_rotation_delta: Quat::IDENTITY,
            carry_yaw: false,
            tick_dt: 1.0 / 60.0,
        },
    );
    Ride {
        registry,
        rider,
        store,
        table,
    }
}

fn foot(registry: &EntityRegistry, rider: EntityId) -> postretro_foundation::pose::FootProbe {
    registry
        .get_component::<MeshComponent>(rider)
        .unwrap()
        .pose_inputs
        .unwrap()
        .feet[0]
}

fn assert_planted(registry: &EntityRegistry, rider: EntityId, alpha: f32) {
    let foot = foot(registry, rider);
    assert!(foot.hit, "alpha {alpha}: the foot finds the drawn deck");
    assert!(
        (foot.contact_height + RIDER_HEIGHT).abs() < BOUND,
        "alpha {alpha}: contact height {} off the drawn deck",
        foot.contact_height
    );
}

// A planted foot on an NPC riding a lift keeps its hit and contact height at
// every alpha on single-player and the host, probed per frame against the lift
// as drawn.
#[test]
fn npc_rider_feet_stay_on_the_drawn_lift_at_every_alpha() {
    let Ride {
        mut registry,
        rider,
        store,
        table,
    } = ride();
    let mut agent = AgentComponent::new(0.4, 1.8, 0.3, 3.0);
    agent.ground = GroundRef::Mover(LIFT);
    registry.set_component(rider, agent).unwrap();
    let mut poses = PresentedMoverPoses::default();
    for alpha in ALPHAS {
        poses.refresh(&table, &registry, alpha);
        update_rider_foot_probes(
            &mut registry,
            &CollisionWorld::new(),
            &[deck()],
            &poses,
            &store,
            0.0,
            alpha,
        );
        assert_planted(&registry, rider, alpha);
    }

    // The live lift pose misses the drawn body mid-tick by more than the bound.
    super::probe_rider(
        &mut registry,
        rider,
        &CollisionWorld::new(),
        &[deck()],
        &table,
        &store,
        0.0,
        0.5,
    );
    let live = foot(&registry, rider).contact_height;
    assert!(
        (live + RIDER_HEIGHT).abs() > BOUND,
        "a probe against the live pose plants off the drawn deck: {live}"
    );
}

// A remote the frame placed on a lift carries no ground of its own on a
// connected client; its presentation pass probes it as a rider.
#[test]
fn rebased_remote_feet_stay_on_the_drawn_lift_at_every_alpha() {
    let Ride {
        mut registry,
        rider,
        store,
        table,
    } = ride();
    let mut poses = PresentedMoverPoses::default();
    for alpha in ALPHAS {
        poses.refresh(&table, &registry, alpha);
        crate::sim::update_presentation_pose_inputs(
            &mut registry,
            &CollisionWorld::new(),
            &[deck()],
            &table,
            &store,
            0.0,
            crate::sim::PresentationPoseInputs {
                camera_aim: (0.0, 0.0),
                remote_player_aims: &HashMap::new(),
                remote_aim_pitches: &HashMap::new(),
                remote_heading_yaws: &HashMap::new(),
                remote_network_ids: &HashMap::new(),
                local_pawn_owns_direction: true,
                riders: RiderFeet::Presented {
                    poses: &poses,
                    alpha,
                    remote_riders: &[rider],
                },
            },
        );
        assert_planted(&registry, rider, alpha);
    }
}

// The fixed tick leaves rider feet to the per-frame pass: it writes nothing.
#[test]
fn fixed_tick_skips_rider_feet() {
    let Ride {
        mut registry,
        rider,
        store,
        table,
    } = ride();
    let mut agent = AgentComponent::new(0.4, 1.8, 0.3, 3.0);
    agent.ground = GroundRef::Mover(LIFT);
    registry.set_component(rider, agent).unwrap();
    let inputs = |registry: &EntityRegistry| {
        registry
            .get_component::<MeshComponent>(rider)
            .unwrap()
            .pose_inputs
    };
    let before = inputs(&registry);
    super::super::pose_inputs::update_foot_ground_probes(
        &mut registry,
        &CollisionWorld::new(),
        &[deck()],
        &table,
        &store,
        0.0,
        RiderFeet::Skip,
    );
    assert_eq!(inputs(&registry), before);
}

// The per-frame rider passes this brief adds allocate nothing once warm: the
// glide advance, presented mover poses and the eye placement.
#[test]
fn per_frame_rider_passes_allocate_nothing_when_warm() {
    use crate::alloc_probe::AllocSnapshot;
    use crate::sim::frame_timing::{EyeBody, EyeMover, InterpolableState, present_eye};

    let Ride {
        mut registry,
        rider,
        store,
        table,
    } = ride();
    let mut agent = AgentComponent::new(0.4, 1.8, 0.3, 3.0);
    agent.ground = GroundRef::Mover(LIFT);
    registry.set_component(rider, agent).unwrap();
    let lift = table.get(LIFT).unwrap().entity;
    registry
        .correct_transform(lift, at_height(RISE_PER_TICK + 0.05), true)
        .unwrap();
    let endpoint = |height: f32| {
        InterpolableState::following(
            Vec3::new(0.0, height + RIDER_HEIGHT + 0.7, 0.0),
            EyeBody {
                centre: Vec3::new(0.0, height + RIDER_HEIGHT, 0.0),
                offset: Vec3::ZERO,
                mover: Some(EyeMover {
                    mover_id: LIFT,
                    entity: lift,
                    pose: at_height(height),
                }),
            },
        )
    };
    let (previous, current) = (endpoint(0.0), endpoint(RISE_PER_TICK + 0.05));
    let deck = [deck()];
    let world = CollisionWorld::new();
    let mut poses = PresentedMoverPoses::default();
    let mut frame = |registry: &mut EntityRegistry, poses: &mut PresentedMoverPoses, alpha: f32| {
        registry.advance_presentation_corrections(0.5, 1.0e-9);
        poses.refresh(&table, registry, alpha);
        update_rider_foot_probes(registry, &world, &deck, poses, &store, 0.0, alpha);
        std::hint::black_box(present_eye(&previous, &current, alpha, |entity| {
            registry.interpolated_transform(entity, alpha).ok()
        }));
    };
    frame(&mut registry, &mut poses, 0.0);
    let probe = AllocSnapshot::arm();
    for step in 0..20 {
        let alpha = step as f32 / 20.0;
        registry.advance_presentation_corrections(0.5, 1.0e-9);
        poses.refresh(&table, &registry, alpha);
        std::hint::black_box(present_eye(&previous, &current, alpha, |entity| {
            registry.interpolated_transform(entity, alpha).ok()
        }));
    }
    assert_eq!(probe.allocs_since(), 0);
    // The rider foot probe itself still allocates inside the shared hit-zone
    // pose sampler (animation clone and joint matrices), reported for the
    // owner rather than asserted: see the rider brief's plan, task 10.
    let probe = AllocSnapshot::arm();
    for step in 0..20 {
        frame(&mut registry, &mut poses, step as f32 / 20.0);
    }
    eprintln!(
        "[riders] rider foot probe allocations: {} per probe",
        probe.allocs_since() / 20
    );
}
