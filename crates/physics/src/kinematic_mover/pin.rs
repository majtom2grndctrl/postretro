//! Host pin test: whether a mover's next course pins a player pawn against
//! static geometry or another mover, and the yield every policy but `crush`
//! answers a pin with.
//! See: context/lib/movement.md §6 · context/lib/networking.md §Current contract

use glam::Vec3;
use parry3d::shape::Capsule;
use postretro_entities::{
    BlockPolicy, ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent,
    Transform,
};
use postretro_foundation::{GroundRef, PlayerMovementComponent};

use super::MoverEventKind;
use super::blocking::{MoverBlockingState, effective_block_policy, pose_has_motion};
use crate::collision::moving::{
    MoverCollider, MoverPose, MoverPoseSource, cast_capsule_against_movers,
    deepest_mover_push_penetration,
};
use crate::collision::{
    CollisionCapsule, CollisionWorld, MAP_UNIT_METRES, SKIN_DISTANCE, cast_capsule_parry_to,
};
use crate::movement::carry_ground_for;

/// A player pawn as the pin test reads it.
#[derive(Debug, Clone, Copy)]
pub(super) struct PawnBody {
    pub(super) entity: EntityId,
    pub(super) position: Vec3,
    pub(super) capsule: Capsule,
    /// Live ground, which the reactive rider exemption reads.
    pub(super) ground: GroundRef,
    /// The ground the next movement tick carries this body with.
    pub(super) carry_ground: GroundRef,
}

/// A pawn the mover's previewed course pins, and the direction the mover
/// drives it: the carry for a rider, the push for any other body.
#[derive(Debug, Clone, Copy)]
pub(super) struct PinnedBody {
    pub(super) direction: Vec3,
    pub(super) rider: bool,
}

/// Every player pawn the pass and the pin test consider. Agents are never
/// carried or displaced, so no mover pins one.
pub(super) fn pawn_bodies(registry: &EntityRegistry) -> Vec<PawnBody> {
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
            Some(PawnBody {
                entity,
                position: transform.position,
                capsule: Capsule::new_y(movement.capsule.half_height, movement.capsule.radius),
                ground: movement.ground,
                carry_ground: carry_ground_for(&movement.movement_state, movement.ground),
            })
        })
        .collect()
}

/// One mover at one pose: a previewed course tested alone.
pub(super) struct OneMoverPose {
    pub(super) mover_id: u32,
    pub(super) pose: MoverPose,
}

impl MoverPoseSource for OneMoverPose {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        (mover_id == self.mover_id).then_some(self.pose)
    }
}

/// The world a mover's course can pin a body against.
pub(super) struct PinWorld<'a> {
    pub(super) static_world: &'a CollisionWorld,
    pub(super) mover_colliders: &'a [MoverCollider],
    /// Every mover at its current pose; other movers are tested there.
    pub(super) mover_poses: &'a dyn MoverPoseSource,
}

impl PinWorld<'_> {
    /// Whether moving `capsule` from `position` by `motion` meets static
    /// geometry or a mover other than `mover_id`, stopping `target_distance`
    /// short of surfaces. Each other mover is swept against by the motion
    /// relative to its last tick of travel, so parts of one machine moving
    /// together never pin a rider between them.
    fn sweep_is_blocked(
        &self,
        mover_id: u32,
        position: Vec3,
        capsule: &Capsule,
        motion: Vec3,
        target_distance: f32,
    ) -> bool {
        if let Some((dir, distance)) = direction_and_length(motion)
            && cast_capsule_parry_to(
                self.static_world,
                position,
                capsule,
                dir,
                distance,
                target_distance,
            )
            .is_some()
        {
            return true;
        }
        self.mover_colliders
            .iter()
            .filter(|other| other.mover_id != mover_id)
            .any(|other| {
                let Some(pose) = self.mover_poses.pose(other.mover_id) else {
                    return false;
                };
                let Some((dir, distance)) = direction_and_length(motion - pose.tick_delta) else {
                    return false;
                };
                cast_capsule_against_movers(
                    std::slice::from_ref(other),
                    &OneMoverPose {
                        mover_id: other.mover_id,
                        pose,
                    },
                    position,
                    capsule,
                    dir,
                    distance,
                    target_distance,
                )
                .is_some()
            })
    }

    /// The pin a mover's pose `preview` (the tick it would run next) makes on
    /// `body`, if any. A rider is pinned when its carry would sweep into a
    /// blocker; any other body when the mover's push on it would.
    pub(super) fn pin(
        &self,
        collider: &MoverCollider,
        preview: MoverPose,
        body: &PawnBody,
    ) -> Option<PinnedBody> {
        let mover_id = collider.mover_id;
        if body.carry_ground == GroundRef::Mover(mover_id) {
            // The chord from `apply_mover_carry`'s start to its end stands in
            // for the tick's arc.
            let pivot = preview.transform.position - preview.tick_delta;
            let carried =
                pivot + preview.tick_rotation_delta * (body.position - pivot) + preview.tick_delta;
            let carry = carried - body.position;
            // One skin width short, as the push test: a contact-only sweep
            // would leave the rider touching a ceiling mover, which then reads
            // that contact as a push when it slides. A rider left under one
            // skin of headroom holds one tick short of its stop, and warns.
            return self
                .sweep_is_blocked(mover_id, body.position, &body.capsule, carry, SKIN_DISTANCE)
                .then(|| PinnedBody {
                    direction: carry.normalize_or_zero(),
                    rider: true,
                });
        }
        let course = OneMoverPose {
            mover_id,
            pose: preview,
        };
        let push = deepest_mover_push_penetration(
            std::slice::from_ref(collider),
            &course,
            body.position,
            &body.capsule,
        )?;
        self.push_is_blocked(mover_id, body, push.normal, push.depth)
            .then_some(PinnedBody {
                direction: push.normal,
                rider: false,
            })
    }

    /// Whether `body` already overlaps static geometry: a crusher's rider it
    /// keeps carrying while embedded stays pinned once the sweep starts inside.
    pub(super) fn is_embedded(&self, body: &PawnBody) -> bool {
        self.static_world.overlaps_static(
            body.position,
            CollisionCapsule::new(body.capsule.radius, body.capsule.half_height()),
        )
    }

    /// Whether a mover's push on `body` along `normal` for `depth` is blocked
    /// by static geometry or another mover. The static half mirrors the
    /// substrate's `mover_push_is_blocked_by_static` skin cast.
    pub(super) fn push_is_blocked(
        &self,
        mover_id: u32,
        body: &PawnBody,
        normal: Vec3,
        depth: f32,
    ) -> bool {
        self.sweep_is_blocked(
            mover_id,
            body.position,
            &body.capsule,
            normal * depth,
            SKIN_DISTANCE,
        )
    }
}

fn direction_and_length(motion: Vec3) -> Option<(Vec3, f32)> {
    let distance = motion.length();
    (distance.is_finite() && distance > f32::EPSILON).then(|| (motion / distance, distance))
}

/// Whether any body could reach the mover within one tick of travel: a cheap
/// bound before the per-mover preview and per-body queries. The collider's
/// reach encloses every rotation about its origin.
pub(super) fn any_body_in_reach(
    collider: &MoverCollider,
    mover: &KinematicMoverComponent,
    origin: Vec3,
    tick_dt: f32,
    bodies: &[PawnBody],
) -> bool {
    let travel = if mover.speed_mps.is_finite() {
        mover.speed_mps.abs() * tick_dt.max(0.0)
    } else {
        f32::INFINITY
    };
    let reach = collider.reach() + travel + SKIN_DISTANCE;
    bodies.iter().any(|body| {
        let body_reach = body.capsule.half_height() + body.capsule.radius;
        let limit = reach + body_reach;
        let distance_squared = body.position.distance_squared(origin);
        // NaN positions fall through to the narrow phase.
        distance_squared <= limit * limit || !distance_squared.is_finite()
    })
}

/// What a non-crush mover does about a pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PinYield {
    Hold,
    Reverse,
}

/// Put a mover on its reversed course, as the `reverse` policy does.
pub(super) fn reverse_course(mover: &mut KinematicMoverComponent) {
    let away = -mover.direction_sign;
    super::reanchor_direction(mover, away);
    mover.target_segment = None;
    mover.started = true;
    mover.completed = false;
    mover.blocked = false;
    mover.wait_remaining_ms = 0.0;
}

/// Whether reversing `mover` gives a course that moves and pins none of
/// `bodies`. A reversal never commits a course no test previewed, and one
/// that goes nowhere (a mover still at its start) is no back-off: it holds.
pub(super) fn reversed_course_is_clear(
    world: &PinWorld<'_>,
    collider: &MoverCollider,
    mover: &KinematicMoverComponent,
    transform: Transform,
    tick_dt: f32,
    bodies: &[PawnBody],
) -> bool {
    let mut reversed = mover.clone();
    reverse_course(&mut reversed);
    let preview = super::preview_next_mover_pose(&reversed, transform, tick_dt, false);
    pose_has_motion(preview)
        && bodies
            .iter()
            .all(|body| world.pin(collider, preview, body).is_none())
}

/// Choose the yield for a pinned mover. `reverse` backs off when its heading
/// drives into every pin and the reversed course pins nothing; anything else
/// holds: a pathless mover, a pin made by spin or off the heading, or a
/// reversal onto another body.
pub(super) fn choose_yield(
    world: &PinWorld<'_>,
    collider: &MoverCollider,
    mover: &KinematicMoverComponent,
    transform: Transform,
    tick_dt: f32,
    pins: &[PinnedBody],
    bodies: &[PawnBody],
) -> PinYield {
    if effective_block_policy(mover) != BlockPolicy::Reverse {
        return PinYield::Hold;
    }
    let Some(heading) = super::blocking::mover_heading_at_tick_end(mover) else {
        return PinYield::Hold;
    };
    let drives_into_every_pin = pins.iter().all(|pin| heading.dot(pin.direction) > 0.0);
    if drives_into_every_pin
        && reversed_course_is_clear(world, collider, mover, transform, tick_dt, bodies)
    {
        PinYield::Reverse
    } else {
        PinYield::Hold
    }
}

/// Apply a yield, fire the Blocked edge on a new pin (`onset`), and warn once
/// per mover per level load when the pin is a rider's.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_yield(
    mover: &mut KinematicMoverComponent,
    entity: EntityId,
    position: Vec3,
    response: PinYield,
    onset: bool,
    pins: &[PinnedBody],
    blocking_state: &mut MoverBlockingState,
    events: &mut Vec<(MoverEventKind, u32)>,
) {
    match response {
        PinYield::Hold => mover.blocked = true,
        PinYield::Reverse => reverse_course(mover),
    }
    if onset {
        events.push((MoverEventKind::Blocked, mover.mover_id));
    }
    if pins.iter().any(|pin| pin.rider) && blocking_state.claim_rider_yield_warning(entity) {
        let map = Vec3::new(-position.z, -position.x, position.y) / MAP_UNIT_METRES;
        log::warn!(
            "[Mover] mover {} yielded to a rider pinned on its course at TrenchBroom ({:.0}, {:.0}, {:.0}); \
             give its riders standing headroom along the path, or author `crush`",
            mover.mover_id,
            map.x,
            map.y,
            map.z,
        );
    }
}

/// Re-test every started mover before the mover tick. Commands applied after
/// the blocking pass (the frame-end script drain, player-event fire lists)
/// reach the driver only along a course that pins nothing: a course into a pin
/// yields again, and a mover already held there fires no second onset.
///
/// This test only adds yields. Release stays with the pass, after movement:
/// nothing moves a pawn between ticks, so a pin the pass released stays
/// released, and a command that cleared a hold already released it.
pub fn run_mover_pin_pretest(
    registry: &mut EntityRegistry,
    static_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    mover_poses: &dyn MoverPoseSource,
    blocking_state: &mut MoverBlockingState,
    tick_dt: f32,
    events: &mut Vec<(MoverEventKind, u32)>,
) {
    let movers: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .map(|(entity, _)| entity)
        .collect();
    if movers.is_empty() {
        return;
    }
    let bodies = pawn_bodies(registry);
    let world = PinWorld {
        static_world,
        mover_colliders,
        mover_poses,
    };
    let mut pins = Vec::new();
    for entity in movers {
        let Ok(transform) = registry.get_component::<Transform>(entity).copied() else {
            continue;
        };
        let Ok(ComponentValue::KinematicMover(mover)) =
            registry.get_component_value_mut(entity, ComponentKind::KinematicMover)
        else {
            continue;
        };
        // A crusher never holds; a hold left from a policy switched to
        // `crush` after the pass releases before this tick moves it.
        if effective_block_policy(mover) == BlockPolicy::Crush {
            mover.blocked = false;
            continue;
        }
        if !mover.started || mover.completed || bodies.is_empty() {
            continue;
        }
        let Some(collider) = mover_colliders
            .iter()
            .find(|collider| collider.mover_id == mover.mover_id)
        else {
            continue;
        };
        if !any_body_in_reach(collider, mover, transform.position, tick_dt, &bodies) {
            continue;
        }
        let preview = super::preview_next_mover_pose(mover, transform, tick_dt, true);
        if !pose_has_motion(preview) {
            continue;
        }
        pins.clear();
        pins.extend(
            bodies
                .iter()
                .filter_map(|body| world.pin(collider, preview, body)),
        );
        if pins.is_empty() {
            continue;
        }
        let response = choose_yield(&world, collider, mover, transform, tick_dt, &pins, &bodies);
        // A mover already held has not been released: no new onset.
        let onset = blocking_state.note_pinned(entity) && !mover.blocked;
        apply_yield(
            mover,
            entity,
            transform.position,
            response,
            onset,
            &pins,
            blocking_state,
            events,
        );
    }
}
