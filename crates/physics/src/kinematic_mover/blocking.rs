//! Host-authoritative kinematic-mover blocking decisions.
//! See: context/lib/entity_model.md §5 · context/lib/networking.md

use std::collections::{HashMap, HashSet};

use parry3d::shape::Capsule;
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, apply_damage_with_context,
};
use postretro_entities::{
    BlockPolicy, ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent,
    Transform,
};
use postretro_foundation::{DamagePayload, GroundRef, PlayerMovementComponent};

use crate::collision::CollisionWorld;
use crate::collision::moving::{
    MoverCollider, MoverPose, MoverPoseSource, deepest_mover_penetration,
    deepest_mover_push_penetration, mover_pose_for_translation_leg,
};
use crate::movement::mover_push_is_blocked_by_static;

// A rider leaves a moving base with jump plus inherited base velocity. 150 ms
// covers the handful of fixed movement ticks needed to clear a mover travelling
// at roughly twice jump velocity, without masking a later path re-entry.
const RIDER_GRACE_MS: f32 = 150.0;

/// Host-local mover transition kinds. Task 6 resolves each entry through the
/// mover's authored named-event address; connected clients never emit entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoverEventKind {
    Opened,
    Closed,
    Blocked,
    Crushed,
}

impl MoverEventKind {
    /// Return this edge's authored named-reaction dispatch address, if any.
    pub fn dispatch_address(self, mover: &KinematicMoverComponent) -> Option<&str> {
        match self {
            Self::Opened => mover.open_event.as_deref(),
            Self::Closed => mover.close_event.as_deref(),
            Self::Blocked => mover.blocked_event.as_deref(),
            Self::Crushed => mover.crush_event.as_deref(),
        }
    }

    /// Return this edge's authored presentation sound key, if any. The key is
    /// descriptor content the app resolves; the simulation never plays it.
    pub fn sound_key(self, mover: &KinematicMoverComponent) -> Option<&str> {
        match self {
            Self::Opened => mover.open_sound.as_deref(),
            Self::Closed => mover.close_sound.as_deref(),
            Self::Blocked => mover.blocked_sound.as_deref(),
            Self::Crushed => mover.crush_sound.as_deref(),
        }
    }
}

/// Host-only blocking timers, keyed by full generation-aware mover and actor
/// identities. They are not component state: peers reconcile outcomes, never
/// these decision timers.
#[derive(Debug, Clone, Default)]
pub struct MoverBlockingState {
    crush_elapsed_ms: HashMap<(EntityId, EntityId), f32>,
    rider_grace_ms: HashMap<(EntityId, EntityId), f32>,
}

#[derive(Debug, Clone, Copy)]
struct MoverPolicySnapshot {
    entity: EntityId,
    mover_id: u32,
    policy: BlockPolicy,
    blocked: bool,
    crush_damage: f32,
    crush_interval_ms: f32,
    heading: Option<glam::Vec3>,
    prospective_pose: Option<MoverPose>,
    policy_active: bool,
}

impl MoverBlockingState {
    /// Discard host-only policy timers when the mover table is reset for a new
    /// level lifetime. Per-tick empty-snapshot cleanup intentionally preserves
    /// rider grace, because it may span a tick with no active movers.
    pub(crate) fn clear(&mut self) {
        self.crush_elapsed_ms.clear();
        self.rider_grace_ms.clear();
    }

    fn has_crush_cadence_for(&self, mover: EntityId) -> bool {
        self.crush_elapsed_ms
            .keys()
            .any(|(cadence_mover, _)| *cadence_mover == mover)
    }

    fn has_crush_cadence(&self, mover: EntityId, victim: EntityId) -> bool {
        self.crush_elapsed_ms.contains_key(&(mover, victim))
    }

    fn has_rider_grace(&self, mover: EntityId, player: EntityId) -> bool {
        self.rider_grace_ms
            .get(&(mover, player))
            .is_some_and(|remaining_ms| *remaining_ms > 0.0)
    }

    #[cfg(test)]
    pub(crate) fn seed_test_cadence(&mut self, mover: EntityId, victim: EntityId) {
        self.crush_elapsed_ms.insert((mover, victim), 75.0);
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.crush_elapsed_ms.is_empty() && self.rider_grace_ms.is_empty()
    }
}

/// Apply the current player and enemy block-policy decisions after player
/// movement and agent steering settle. The decision is deliberately outside
/// prediction: a connected client only reconciles the resulting mover phase and
/// health.
#[allow(clippy::too_many_arguments)]
pub fn run_mover_blocking_pass(
    registry: &mut EntityRegistry,
    static_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    mover_poses: &dyn MoverPoseSource,
    blocking_state: &mut MoverBlockingState,
    tick_dt: f32,
    events: &mut Vec<(MoverEventKind, u32)>,
    on_impact: &mut impl FnMut(&mut EntityRegistry),
) {
    let stale_blocked: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .filter_map(|(entity, value)| {
            let ComponentValue::KinematicMover(mover) = value else {
                return None;
            };
            let policy = effective_block_policy(mover);
            (mover.blocked
                && (policy != BlockPolicy::Stop
                    || !policy_is_active_this_tick(mover, policy, mover_poses, None)))
            .then_some(entity)
        })
        .collect();
    for entity in stale_blocked {
        if let Ok(ComponentValue::KinematicMover(mover)) =
            registry.get_component_value_mut(entity, ComponentKind::KinematicMover)
        {
            mover.blocked = false;
        }
    }

    let movers: Vec<MoverPolicySnapshot> = registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .filter_map(|(entity, value)| {
            let ComponentValue::KinematicMover(mover) = value else {
                return None;
            };
            let policy = effective_block_policy(mover);
            let transform = registry.get_component::<Transform>(entity).ok().copied()?;
            let prospective_pose = (mover.started && !mover.completed)
                .then(|| {
                    super::preview_next_mover_pose(
                        mover,
                        transform,
                        tick_dt,
                        policy == BlockPolicy::Stop && mover.blocked,
                    )
                })
                .filter(|pose| pose_has_motion(*pose));
            let policy_active =
                policy_is_active_this_tick(mover, policy, mover_poses, prospective_pose);
            let maintains_crush_cadence =
                policy == BlockPolicy::Crush && blocking_state.has_crush_cadence_for(entity);
            (policy != BlockPolicy::Displace && (policy_active || maintains_crush_cadence)).then(
                || MoverPolicySnapshot {
                    entity,
                    mover_id: mover.mover_id,
                    policy,
                    blocked: mover.blocked,
                    crush_damage: mover.crush_damage,
                    crush_interval_ms: mover.crush_interval_ms,
                    heading: mover_heading_at_tick_end(mover),
                    prospective_pose,
                    policy_active,
                },
            )
        })
        .collect();
    let player_capsules = player_capsules(registry);
    refresh_rider_grace(blocking_state, registry, &player_capsules, tick_dt);
    if movers.is_empty() {
        blocking_state.crush_elapsed_ms.clear();
        return;
    }

    let agent_capsules = agent_capsules(registry);
    let mut contacted_stop_movers = HashSet::new();
    let mut reverse_contacts: HashMap<EntityId, glam::Vec3> = HashMap::new();
    let mut pinned_crush_contacts = HashSet::new();

    for mover in &movers {
        let Some(collider) = mover_colliders
            .iter()
            .find(|collider| collider.mover_id == mover.mover_id)
        else {
            continue;
        };

        for (player_entity, position, capsule, player_ground) in &player_capsules {
            let contact = leading_mover_contact_penetration(
                collider,
                mover_poses,
                mover.prospective_pose,
                *position,
                capsule,
            );

            match mover.policy {
                BlockPolicy::Stop | BlockPolicy::Reverse => {
                    let Some(contact) = contact else {
                        continue;
                    };
                    if *player_ground == GroundRef::Mover(mover.mover_id)
                        || (*player_ground == GroundRef::Airborne
                            && blocking_state.has_rider_grace(mover.entity, *player_entity))
                    {
                        continue;
                    }
                    note_reactive_contact(
                        mover.entity,
                        mover,
                        contact.normal,
                        &mut contacted_stop_movers,
                        &mut reverse_contacts,
                    );
                }
                BlockPolicy::Crush => {
                    let maintains_cadence =
                        blocking_state.has_crush_cadence(mover.entity, *player_entity);
                    if contact.is_none() && !maintains_cadence {
                        continue;
                    }
                    if !mover.policy_active && !maintains_cadence {
                        continue;
                    }
                    // Contact is conservative, but pinning must use the actual
                    // player capsule and the same static-push predicate as local
                    // mover displacement. A held cadence keeps sampling the
                    // actual overlap: its stationary mesh contact has no swept
                    // push direction to re-derive the original static block.
                    if let Some(actual_penetration) = deepest_mover_push_penetration(
                        std::slice::from_ref(collider),
                        mover_poses,
                        *position,
                        capsule,
                    ) && mover_push_is_blocked_by_static(
                        static_world,
                        *position,
                        capsule,
                        actual_penetration,
                    ) {
                        pinned_crush_contacts.insert((mover.entity, *player_entity));
                    } else if maintains_cadence
                        && deepest_mover_penetration(
                            std::slice::from_ref(collider),
                            mover_poses,
                            *position,
                            capsule,
                        )
                        .is_some()
                    {
                        pinned_crush_contacts.insert((mover.entity, *player_entity));
                    }
                }
                BlockPolicy::Displace => unreachable!("displace movers do not enter the pass"),
            }
        }

        for (enemy_entity, position, capsule) in &agent_capsules {
            let contact = leading_mover_contact_penetration(
                collider,
                mover_poses,
                mover.prospective_pose,
                *position,
                capsule,
            );

            match mover.policy {
                BlockPolicy::Stop | BlockPolicy::Reverse => {
                    let Some(contact) = contact else {
                        continue;
                    };
                    note_reactive_contact(
                        mover.entity,
                        mover,
                        contact.normal,
                        &mut contacted_stop_movers,
                        &mut reverse_contacts,
                    );
                }
                // Agents never take the player-only mover-displacement path, so
                // an actual overlap is necessarily a pinned crusher contact.
                BlockPolicy::Crush => {
                    let maintains_cadence =
                        blocking_state.has_crush_cadence(mover.entity, *enemy_entity);
                    if contact.is_none() && !maintains_cadence {
                        continue;
                    }
                    if !mover.policy_active && !maintains_cadence {
                        continue;
                    }
                    if deepest_mover_push_penetration(
                        std::slice::from_ref(collider),
                        mover_poses,
                        *position,
                        capsule,
                    )
                    .is_some()
                    {
                        pinned_crush_contacts.insert((mover.entity, *enemy_entity));
                    }
                }
                BlockPolicy::Displace => unreachable!("displace movers do not enter the pass"),
            }
        }
    }

    for snapshot in movers {
        let stop_contact = snapshot.policy == BlockPolicy::Stop
            && contacted_stop_movers.contains(&snapshot.entity);
        if stop_contact && !snapshot.blocked {
            events.push((MoverEventKind::Blocked, snapshot.mover_id));
        }

        if let Ok(ComponentValue::KinematicMover(mover)) =
            registry.get_component_value_mut(snapshot.entity, ComponentKind::KinematicMover)
        {
            mover.blocked = stop_contact;
            if snapshot.policy == BlockPolicy::Reverse
                && let Some(contact_normal) = reverse_contacts.get(&snapshot.entity)
                && snapshot
                    .heading
                    .is_some_and(|heading| heading.dot(*contact_normal) > 0.0)
            {
                let direction_away_from_contact = -mover.direction_sign;
                super::reanchor_direction(mover, direction_away_from_contact);
                mover.target_segment = None;
                mover.started = true;
                mover.completed = false;
                mover.wait_remaining_ms = 0.0;
                events.push((MoverEventKind::Blocked, mover.mover_id));
            }
        }

        if snapshot.policy == BlockPolicy::Crush {
            for &(crush_mover, victim) in &pinned_crush_contacts {
                if crush_mover != snapshot.entity || registry.is_inventory_owned(victim) {
                    continue;
                }
                if !crush_hit_is_due(
                    blocking_state,
                    (snapshot.entity, victim),
                    snapshot.crush_interval_ms,
                    tick_dt,
                ) {
                    continue;
                }
                if !apply_damage_with_context(
                    registry,
                    victim,
                    &DamagePayload {
                        amount: snapshot.crush_damage,
                        impulse: glam::Vec3::ZERO,
                    },
                    DamageContext::new("mover.crush", DamageProducer::InTick),
                ) {
                    continue;
                }
                on_impact(registry);
                events.push((MoverEventKind::Crushed, snapshot.mover_id));
            }
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
    blocking_state
        .rider_grace_ms
        .retain(|(mover, player), remaining_ms| {
            *remaining_ms > 0.0
                && registry
                    .get_component::<KinematicMoverComponent>(*mover)
                    .is_ok()
                && registry
                    .get_component::<PlayerMovementComponent>(*player)
                    .is_ok()
        });
}

fn refresh_rider_grace(
    blocking_state: &mut MoverBlockingState,
    registry: &EntityRegistry,
    players: &[(EntityId, glam::Vec3, Capsule, GroundRef)],
    tick_dt: f32,
) {
    let tick_ms = (tick_dt * 1000.0).max(0.0);
    blocking_state
        .rider_grace_ms
        .retain(|(mover, player), remaining_ms| {
            let Ok(mover) = registry.get_component::<KinematicMoverComponent>(*mover) else {
                return false;
            };
            let Ok(player) = registry.get_component::<PlayerMovementComponent>(*player) else {
                return false;
            };
            match player.ground {
                GroundRef::Mover(ground_mover_id) if ground_mover_id == mover.mover_id => {
                    *remaining_ms = RIDER_GRACE_MS;
                    true
                }
                GroundRef::Mover(_) | GroundRef::World => false,
                GroundRef::Airborne => {
                    *remaining_ms -= tick_ms;
                    *remaining_ms > 0.0
                }
            }
        });

    for (player, _, _, ground) in players {
        let GroundRef::Mover(mover_id) = ground else {
            continue;
        };
        let Some(mover) = registry
            .iter_with_kind(ComponentKind::KinematicMover)
            .find_map(|(entity, value)| {
                let ComponentValue::KinematicMover(mover) = value else {
                    return None;
                };
                (mover.mover_id == *mover_id).then_some(entity)
            })
        else {
            continue;
        };

        blocking_state
            .rider_grace_ms
            .entry((mover, *player))
            .or_insert(RIDER_GRACE_MS);
    }
}

fn note_reactive_contact(
    mover_entity: EntityId,
    mover: &MoverPolicySnapshot,
    contact_normal: glam::Vec3,
    contacted_stop_movers: &mut HashSet<EntityId>,
    reverse_contacts: &mut HashMap<EntityId, glam::Vec3>,
) {
    match mover.policy {
        BlockPolicy::Stop => {
            contacted_stop_movers.insert(mover_entity);
        }
        BlockPolicy::Reverse => {
            // The driver has already advanced (and trigger commands have
            // already run), so this heading is the live tick-end intent.
            // `tick_delta` would be stale at a corner or post-terminus
            // auto-reversal and can therefore reverse the wrong way.
            if mover
                .heading
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
    if mover.block_policy == BlockPolicy::Reverse && mover.waypoints.len() < 2 {
        BlockPolicy::Stop
    } else {
        mover.block_policy
    }
}

fn policy_is_active_this_tick(
    mover: &KinematicMoverComponent,
    policy: BlockPolicy,
    mover_poses: &dyn MoverPoseSource,
    prospective_pose: Option<MoverPose>,
) -> bool {
    if policy == BlockPolicy::Stop && mover.blocked && mover.started && !mover.completed {
        // A held stop mover must keep sampling contact so it can resume when
        // clear even though its published motion is intentionally zero.
        return true;
    }
    if mover_poses.had_endpoint_arrival(mover.mover_id) {
        return true;
    }
    mover_poses
        .pose(mover.mover_id)
        .is_some_and(pose_has_motion)
        || prospective_pose.is_some_and(pose_has_motion)
}

fn pose_has_motion(pose: MoverPose) -> bool {
    let translated = pose.tick_delta.is_finite()
        && pose.tick_delta.length_squared() > f32::EPSILON * f32::EPSILON;
    let rotation = pose.tick_rotation_delta.to_scaled_axis();
    let rotated = rotation.is_finite() && rotation.length_squared() > f32::EPSILON * f32::EPSILON;
    let spinning = pose.angular_velocity.is_finite()
        && pose.angular_velocity.length_squared() > f32::EPSILON * f32::EPSILON;
    translated || rotated || spinning
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

fn player_capsules(registry: &EntityRegistry) -> Vec<(EntityId, glam::Vec3, Capsule, GroundRef)> {
    registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .filter_map(|(entity, _)| {
            if registry.is_inventory_owned(entity) {
                return None;
            }
            let movement = registry
                .get_component::<PlayerMovementComponent>(entity)
                .ok()?;
            let transform = registry.get_component::<Transform>(entity).ok()?;
            Some((
                entity,
                transform.position,
                Capsule::new_y(movement.capsule.half_height, movement.capsule.radius),
                movement.ground,
            ))
        })
        .collect()
}

fn agent_capsules(registry: &EntityRegistry) -> Vec<(EntityId, glam::Vec3, Capsule)> {
    registry
        .iter_with_kind(ComponentKind::Agent)
        .filter_map(|(entity, _)| {
            if registry.is_inventory_owned(entity) {
                return None;
            }
            let agent = registry.get_component::<AgentComponent>(entity).ok()?;
            let transform = registry.get_component::<Transform>(entity).ok()?;
            Some((
                entity,
                transform.position,
                Capsule::new_y(agent.half_height(), agent.radius),
            ))
        })
        .collect()
}

fn is_blocking_victim(registry: &EntityRegistry, entity: EntityId) -> bool {
    if registry.is_inventory_owned(entity) {
        return false;
    }
    registry
        .has_component_kind(entity, ComponentKind::PlayerMovement)
        .unwrap_or(false)
        || registry
            .has_component_kind(entity, ComponentKind::Agent)
            .unwrap_or(false)
}

/// Query contact made this tick, then extend only the mover's leading sweep by
/// one more tick. Expanding capsule radius would also grow behind and beside
/// the mover, producing policy reactions where no leading face can arrive.
fn leading_mover_contact_penetration(
    collider: &MoverCollider,
    mover_poses: &dyn MoverPoseSource,
    prospective_pose: Option<MoverPose>,
    position: glam::Vec3,
    capsule: &Capsule,
) -> Option<crate::collision::moving::MoverPenetration> {
    if let Some(legs) = mover_poses.translation_legs(collider.mover_id)
        && !legs.is_empty()
    {
        if let Some(contact) = deepest_mover_penetration(
            std::slice::from_ref(collider),
            mover_poses,
            position,
            capsule,
        ) {
            return Some(contact);
        }
        let base_pose = mover_poses.pose(collider.mover_id)?;
        let mut latest_contact = None;
        for leg in legs {
            let leg_pose = mover_pose_for_translation_leg(base_pose, *leg);
            let leg_source = ProspectiveMoverPose {
                mover_id: collider.mover_id,
                pose: leg_pose,
            };
            if let Some(contact) = deepest_mover_push_penetration(
                std::slice::from_ref(collider),
                &leg_source,
                position,
                capsule,
            ) {
                latest_contact = Some(contact);
            }
        }
        if latest_contact.is_some() {
            return latest_contact;
        }
    } else {
        let contact = deepest_mover_push_penetration(
            std::slice::from_ref(collider),
            mover_poses,
            position,
            capsule,
        );
        if contact.is_some() {
            return contact;
        }
    }

    let prospective_pose = prospective_pose.filter(|pose| pose_has_motion(*pose))?;
    let prospective_source = ProspectiveMoverPose {
        mover_id: collider.mover_id,
        pose: prospective_pose,
    };
    deepest_mover_push_penetration(
        std::slice::from_ref(collider),
        &prospective_source,
        position,
        capsule,
    )
}

struct ProspectiveMoverPose {
    mover_id: u32,
    pose: crate::collision::moving::MoverPose,
}

impl MoverPoseSource for ProspectiveMoverPose {
    fn pose(&self, mover_id: u32) -> Option<crate::collision::moving::MoverPose> {
        (mover_id == self.mover_id).then_some(self.pose)
    }
}

#[cfg(test)]
#[path = "blocking_tests.rs"]
mod tests;
