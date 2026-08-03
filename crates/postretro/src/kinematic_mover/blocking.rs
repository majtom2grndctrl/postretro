//! Host-authoritative kinematic-mover blocking decisions.
//!
//! The shared mover driver consumes only the resulting `blocked` phase bit;
//! this pass deliberately runs only from the authoritative simulation seam.

use std::collections::HashSet;

use parry3d::math::Point;
use parry3d::shape::Capsule;
use postretro_entities::{
    BlockPolicy, ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent,
    Transform,
};
use postretro_foundation::PlayerMovementComponent;

use crate::collision::CollisionWorld;
use crate::collision::moving::{MoverCollider, MoverPoseSource, deepest_mover_push_penetration};
use crate::movement::mover_push_is_blocked_by_static;

/// Host-local mover transition kinds. Task 6 resolves each entry through the
/// mover's authored named-event address; connected clients never emit entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoverEventKind {
    Opened,
    Closed,
    Blocked,
    #[allow(dead_code)] // Task 2's crusher policy owns production emission.
    Crushed,
}

/// Apply the current stop-policy contact decisions after player movement and
/// agent steering settle. The decision is deliberately outside prediction: a
/// connected client only reconciles the `blocked` phase it receives.
pub(crate) fn run_mover_blocking_pass(
    registry: &mut EntityRegistry,
    static_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    mover_poses: &dyn MoverPoseSource,
    events: &mut Vec<(MoverEventKind, u32)>,
) {
    let movers: Vec<(EntityId, u32, BlockPolicy, bool)> = registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .filter_map(|(entity, value)| {
            let ComponentValue::KinematicMover(mover) = value else {
                return None;
            };
            Some((entity, mover.mover_id, mover.block_policy, mover.blocked))
        })
        .collect();
    if movers.is_empty() {
        return;
    }

    let stop_mover_ids: HashSet<u32> = movers
        .iter()
        .filter_map(|(_, mover_id, policy, _)| (*policy == BlockPolicy::Stop).then_some(*mover_id))
        .collect();
    let mut contacted_stop_movers = HashSet::new();
    if !stop_mover_ids.is_empty() {
        for (position, capsule) in player_capsules(registry) {
            for collider in mover_colliders {
                if !stop_mover_ids.contains(&collider.mover_id) {
                    continue;
                }
                let capsule =
                    conservatively_inflated_contact_capsule(&capsule, collider, mover_poses);
                if let Some(penetration) = deepest_mover_push_penetration(
                    std::slice::from_ref(collider),
                    mover_poses,
                    Point::new(position.x, position.y, position.z),
                    &capsule,
                ) {
                    // Stop reacts to every player contact, not only to a pinch.
                    // Retain the shared static-push classification at this host
                    // boundary so the reverse/crush policies can branch on the
                    // same predicate without duplicating geometry semantics.
                    let _pinched_against_static = mover_push_is_blocked_by_static(
                        static_world,
                        position,
                        &capsule,
                        penetration,
                    );
                    contacted_stop_movers.insert(collider.mover_id);
                }
            }
        }
    }

    for (entity, mover_id, policy, was_blocked) in movers {
        let blocked = policy == BlockPolicy::Stop && contacted_stop_movers.contains(&mover_id);
        if blocked && !was_blocked {
            events.push((MoverEventKind::Blocked, mover_id));
        }
        if blocked == was_blocked {
            continue;
        }
        let Ok(mut mover) = registry
            .get_component::<KinematicMoverComponent>(entity)
            .cloned()
        else {
            continue;
        };
        mover.blocked = blocked;
        let _ = registry.set_component(entity, mover);
    }
}

fn player_capsules(registry: &EntityRegistry) -> Vec<(glam::Vec3, Capsule)> {
    registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .filter_map(|(entity, _)| {
            let movement = registry
                .get_component::<PlayerMovementComponent>(entity)
                .ok()?;
            let transform = registry.get_component::<Transform>(entity).ok()?;
            Some((
                transform.position,
                Capsule::new(
                    Point::new(0.0, -movement.capsule.half_height, 0.0),
                    Point::new(0.0, movement.capsule.half_height, 0.0),
                    movement.capsule.radius,
                ),
            ))
        })
        .collect()
}

/// The pass runs after mover motion has already been applied, so extend the
/// player-contact shape by one tick of the candidate mover's linear travel.
/// This is conservative by design: producer-to-next-tick latency cannot allow a
/// fast leading face to visibly sink into a capsule before the next driver pass.
fn conservatively_inflated_contact_capsule(
    capsule: &Capsule,
    collider: &MoverCollider,
    mover_poses: &dyn MoverPoseSource,
) -> Capsule {
    let leading_face_inflation = mover_poses
        .pose(collider.mover_id)
        .map(|pose| pose.tick_delta.length())
        .filter(|distance| distance.is_finite())
        .unwrap_or(0.0);
    Capsule::new(
        capsule.segment.a,
        capsule.segment.b,
        capsule.radius + leading_face_inflation,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec3};
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

    fn player_movement() -> PlayerMovementComponent {
        PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
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

    #[test]
    fn stop_policy_holds_on_swept_player_contact_and_clears_after_contact() {
        let mover_id = 42;
        let mut registry = EntityRegistry::new();
        let mover_entity = registry.spawn(Transform::default());
        let mut mover = KinematicMoverComponent::new(
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
        );
        mover.block_policy = BlockPolicy::Stop;
        registry.set_component(mover_entity, mover).unwrap();
        let player = registry.spawn(Transform {
            position: Vec3::new(0.0, 1.0, 0.0),
            ..Transform::default()
        });
        registry.set_component(player, player_movement()).unwrap();

        let collider = swept_wall(mover_id);
        let static_world = CollisionWorld::new();
        let contacting_poses = SingleMoverPose {
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
        };
        let mut events = Vec::new();

        run_mover_blocking_pass(
            &mut registry,
            &static_world,
            std::slice::from_ref(&collider),
            &contacting_poses,
            &mut events,
        );
        assert!(
            registry
                .get_component::<KinematicMoverComponent>(mover_entity)
                .unwrap()
                .blocked
        );
        assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);

        let clear_poses = SingleMoverPose {
            mover_id,
            pose: crate::collision::moving::MoverPose {
                transform: Transform {
                    position: Vec3::new(10.0, 0.0, 0.0),
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::ZERO,
                tick_rotation_delta: Quat::IDENTITY,
                carry_yaw: false,
                tick_dt: 0.1,
            },
        };
        run_mover_blocking_pass(
            &mut registry,
            &static_world,
            std::slice::from_ref(&collider),
            &clear_poses,
            &mut events,
        );
        assert!(
            !registry
                .get_component::<KinematicMoverComponent>(mover_entity)
                .unwrap()
                .blocked
        );
        assert_eq!(events, vec![(MoverEventKind::Blocked, mover_id)]);
    }
}
