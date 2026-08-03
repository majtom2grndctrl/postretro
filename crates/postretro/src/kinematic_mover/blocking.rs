//! Host-authoritative kinematic-mover blocking decisions.
//! See: context/lib/entity_model.md §5 · context/lib/networking.md

use std::collections::{HashMap, HashSet};

use parry3d::math::Point;
use parry3d::shape::Capsule;
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, apply_damage_with_context,
};
use postretro_entities::{
    BlockPolicy, ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent,
    Transform,
};
use postretro_foundation::{DamagePayload, PlayerMovementComponent};

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
    Crushed,
}

impl MoverEventKind {
    /// Return this edge's authored named-reaction dispatch address, if any.
    pub(crate) fn dispatch_address(self, mover: &KinematicMoverComponent) -> Option<&str> {
        match self {
            Self::Opened => mover.open_event.as_deref(),
            Self::Closed => mover.close_event.as_deref(),
            Self::Blocked => mover.blocked_event.as_deref(),
            Self::Crushed => mover.crush_event.as_deref(),
        }
    }
}

/// Host-only crush clocks, keyed by the full generation-aware identities of
/// both the mover and its victim. They are intentionally not component state:
/// peers reconcile motion and health, never this decision cadence.
#[derive(Debug, Clone, Default)]
pub(crate) struct MoverBlockingState {
    crush_elapsed_ms: HashMap<(EntityId, EntityId), f32>,
}

/// Apply the current player and enemy block-policy decisions after player
/// movement and agent steering settle. The decision is deliberately outside
/// prediction: a connected client only reconciles the resulting mover phase and
/// health.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_mover_blocking_pass(
    registry: &mut EntityRegistry,
    static_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    mover_poses: &dyn MoverPoseSource,
    blocking_state: &mut MoverBlockingState,
    tick_dt: f32,
    events: &mut Vec<(MoverEventKind, u32)>,
    on_impact: &mut impl FnMut(&mut EntityRegistry),
) {
    let movers: Vec<(EntityId, KinematicMoverComponent)> = registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .filter_map(|(entity, value)| {
            let ComponentValue::KinematicMover(mover) = value else {
                return None;
            };
            Some((entity, mover.clone()))
        })
        .collect();
    if movers.is_empty() {
        blocking_state.crush_elapsed_ms.clear();
        return;
    }

    let player_capsules = player_capsules(registry);
    let agent_capsules = agent_capsules(registry);
    let mut contacted_stop_movers = HashSet::new();
    let mut reverse_contacts: HashMap<EntityId, glam::Vec3> = HashMap::new();
    let mut pinned_crush_contacts = HashSet::new();

    for (mover_entity, mover) in &movers {
        let effective_policy = effective_block_policy(mover);
        if effective_policy == BlockPolicy::Displace {
            continue;
        }
        let Some(collider) = mover_colliders
            .iter()
            .find(|collider| collider.mover_id == mover.mover_id)
        else {
            continue;
        };

        for (player_entity, position, capsule) in &player_capsules {
            let contact_capsule =
                conservatively_inflated_contact_capsule(capsule, collider, mover_poses);
            let Some(penetration) = deepest_mover_push_penetration(
                std::slice::from_ref(collider),
                mover_poses,
                Point::new(position.x, position.y, position.z),
                &contact_capsule,
            ) else {
                continue;
            };

            match effective_policy {
                BlockPolicy::Stop | BlockPolicy::Reverse => note_reactive_contact(
                    *mover_entity,
                    mover,
                    effective_policy,
                    penetration.normal,
                    &mut contacted_stop_movers,
                    &mut reverse_contacts,
                ),
                BlockPolicy::Crush => {
                    // Contact is conservative, but pinning must use the actual
                    // player capsule and the same static-push predicate as local
                    // mover displacement. A player that can clear takes no hit.
                    if let Some(actual_penetration) = deepest_mover_push_penetration(
                        std::slice::from_ref(collider),
                        mover_poses,
                        Point::new(position.x, position.y, position.z),
                        capsule,
                    ) && mover_push_is_blocked_by_static(
                        static_world,
                        *position,
                        capsule,
                        actual_penetration,
                    ) {
                        pinned_crush_contacts.insert((*mover_entity, *player_entity));
                    }
                }
                BlockPolicy::Displace => unreachable!("displace movers do not enter the pass"),
            }
        }

        for (enemy_entity, position, capsule) in &agent_capsules {
            let contact_capsule =
                conservatively_inflated_contact_capsule(capsule, collider, mover_poses);
            let Some(penetration) = deepest_mover_push_penetration(
                std::slice::from_ref(collider),
                mover_poses,
                Point::new(position.x, position.y, position.z),
                &contact_capsule,
            ) else {
                continue;
            };

            match effective_policy {
                BlockPolicy::Stop | BlockPolicy::Reverse => note_reactive_contact(
                    *mover_entity,
                    mover,
                    effective_policy,
                    penetration.normal,
                    &mut contacted_stop_movers,
                    &mut reverse_contacts,
                ),
                // Agents never take the player-only mover-displacement path, so
                // an actual overlap is necessarily a pinned crusher contact.
                BlockPolicy::Crush => {
                    if deepest_mover_push_penetration(
                        std::slice::from_ref(collider),
                        mover_poses,
                        Point::new(position.x, position.y, position.z),
                        capsule,
                    )
                    .is_some()
                    {
                        pinned_crush_contacts.insert((*mover_entity, *enemy_entity));
                    }
                }
                BlockPolicy::Displace => unreachable!("displace movers do not enter the pass"),
            }
        }
    }

    for (entity, mover_snapshot) in movers {
        let effective_policy = effective_block_policy(&mover_snapshot);
        let stop_contact =
            effective_policy == BlockPolicy::Stop && contacted_stop_movers.contains(&entity);
        if stop_contact && !mover_snapshot.blocked {
            events.push((MoverEventKind::Blocked, mover_snapshot.mover_id));
        }

        let mut mover = mover_snapshot.clone();
        mover.blocked = stop_contact;

        if effective_policy == BlockPolicy::Reverse
            && let Some(contact_normal) = reverse_contacts.get(&entity)
            && mover_heading_at_tick_end(&mover)
                .is_some_and(|heading| heading.dot(*contact_normal) > 0.0)
        {
            let direction_away_from_contact = -mover.direction_sign;
            super::reanchor_direction(&mut mover, direction_away_from_contact);
            mover.target_segment = None;
            mover.started = true;
            mover.completed = false;
            mover.wait_remaining_ms = 0.0;
            events.push((MoverEventKind::Blocked, mover.mover_id));
        }

        if effective_policy == BlockPolicy::Crush {
            for &(crush_mover, victim) in &pinned_crush_contacts {
                if crush_mover != entity {
                    continue;
                }
                if !crush_hit_is_due(
                    blocking_state,
                    (entity, victim),
                    mover.crush_interval_ms,
                    tick_dt,
                ) {
                    continue;
                }
                apply_damage_with_context(
                    registry,
                    victim,
                    &DamagePayload {
                        amount: mover.crush_damage,
                    },
                    DamageContext::new("mover.crush", DamageProducer::InTick),
                );
                on_impact(registry);
                events.push((MoverEventKind::Crushed, mover.mover_id));
            }
        }

        if mover != mover_snapshot {
            let _ = registry.set_component(entity, mover);
        }
    }

    blocking_state
        .crush_elapsed_ms
        .retain(|(mover, victim), _| {
            pinned_crush_contacts.contains(&(*mover, *victim))
                && registry
                    .get_component::<KinematicMoverComponent>(*mover)
                    .is_ok()
                && is_blocking_victim(registry, *victim)
        });
}

fn note_reactive_contact(
    mover_entity: EntityId,
    mover: &KinematicMoverComponent,
    effective_policy: BlockPolicy,
    contact_normal: glam::Vec3,
    contacted_stop_movers: &mut HashSet<EntityId>,
    reverse_contacts: &mut HashMap<EntityId, glam::Vec3>,
) {
    match effective_policy {
        BlockPolicy::Stop => {
            contacted_stop_movers.insert(mover_entity);
        }
        BlockPolicy::Reverse => {
            // The driver has already advanced (and trigger commands have
            // already run), so this heading is the live tick-end intent.
            // `tick_delta` would be stale at a corner or post-terminus
            // auto-reversal and can therefore reverse the wrong way.
            if mover_heading_at_tick_end(mover)
                .is_some_and(|heading| heading.dot(contact_normal) > 0.0)
            {
                reverse_contacts
                    .entry(mover_entity)
                    .or_insert(contact_normal);
            }
        }
        BlockPolicy::Displace | BlockPolicy::Crush => {
            unreachable!("only stop and reverse contacts are reactive")
        }
    }
}

fn effective_block_policy(mover: &KinematicMoverComponent) -> BlockPolicy {
    // A pathless mover cannot reverse; preserve the normal stop hold, including
    // its replicated `blocked` phase, instead of manufacturing direction state.
    (mover.block_policy == BlockPolicy::Reverse && mover.waypoints.len() < 2)
        .then_some(BlockPolicy::Stop)
        .unwrap_or(mover.block_policy)
}

fn mover_heading_at_tick_end(mover: &KinematicMoverComponent) -> Option<glam::Vec3> {
    let from = usize::from(mover.segment_index);
    let to = if mover.direction_sign >= 0 {
        from.checked_add(1)?
    } else {
        from.checked_sub(1)?
    };
    let heading = (*mover.waypoints.get(to)? - *mover.waypoints.get(from)?).normalize_or_zero();
    (heading.is_finite() && heading != glam::Vec3::ZERO).then_some(heading)
}

fn crush_hit_is_due(
    blocking_state: &mut MoverBlockingState,
    key: (EntityId, EntityId),
    interval_ms: f32,
    tick_dt: f32,
) -> bool {
    let tick_ms = (tick_dt * 1000.0).max(0.0);
    if interval_ms <= tick_ms {
        blocking_state.crush_elapsed_ms.insert(key, 0.0);
        return true;
    }
    let Some(elapsed_ms) = blocking_state.crush_elapsed_ms.get_mut(&key) else {
        blocking_state.crush_elapsed_ms.insert(key, 0.0);
        return true;
    };
    *elapsed_ms += tick_ms;
    if *elapsed_ms < interval_ms {
        return false;
    }
    *elapsed_ms -= interval_ms;
    true
}

fn player_capsules(registry: &EntityRegistry) -> Vec<(EntityId, glam::Vec3, Capsule)> {
    registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .filter_map(|(entity, _)| {
            let movement = registry
                .get_component::<PlayerMovementComponent>(entity)
                .ok()?;
            let transform = registry.get_component::<Transform>(entity).ok()?;
            Some((
                entity,
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

fn agent_capsules(registry: &EntityRegistry) -> Vec<(EntityId, glam::Vec3, Capsule)> {
    registry
        .iter_with_kind(ComponentKind::Agent)
        .filter_map(|(entity, _)| {
            let agent = registry.get_component::<AgentComponent>(entity).ok()?;
            let transform = registry.get_component::<Transform>(entity).ok()?;
            Some((
                entity,
                transform.position,
                Capsule::new(
                    Point::new(0.0, -agent.half_height(), 0.0),
                    Point::new(0.0, agent.half_height(), 0.0),
                    agent.radius,
                ),
            ))
        })
        .collect()
}

fn is_blocking_victim(registry: &EntityRegistry, entity: EntityId) -> bool {
    registry
        .has_component_kind(entity, ComponentKind::PlayerMovement)
        .unwrap_or(false)
        || registry
            .has_component_kind(entity, ComponentKind::Agent)
            .unwrap_or(false)
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
    use parry3d::{math::Isometry, shape::TriMesh};
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
            position: Vec3::new(0.0, 1.0, 0.0),
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

    fn add_enemy(registry: &mut EntityRegistry, health: Option<f32>) -> EntityId {
        let enemy = registry.spawn(Transform {
            position: Vec3::new(0.0, 1.0, 0.0),
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
            Point::new(0.1, 0.0, -1.0),
            Point::new(0.1, 2.0, -1.0),
            Point::new(0.1, 2.0, 1.0),
            Point::new(0.1, 0.0, 1.0),
        ];
        CollisionWorld {
            mesh: TriMesh::new(points, vec![[0, 1, 2], [0, 2, 3]]),
            isometry: Isometry::identity(),
        }
    }

    #[test]
    fn stop_policy_holds_on_swept_player_contact_and_clears_after_contact() {
        let mover_id = 42;
        let mut registry = EntityRegistry::new();
        let mover_entity = registry.spawn(Transform::default());
        let mut mover = mover(mover_id);
        mover.block_policy = BlockPolicy::Stop;
        registry.set_component(mover_entity, mover).unwrap();
        add_player(&mut registry, None);

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
    fn crush_policy_skips_player_that_can_clear_the_push() {
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
        drop(run_pass);

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
        crate::scripting_systems::health::sweep_deaths(&mut registry);
        run_pass(&mut registry);
        run_pass(&mut registry); // The latched enemy still receives overkill damage.
        drop(run_pass);

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
        drop(run_pass);

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
}
