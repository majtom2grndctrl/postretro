// Mover carry, release velocity and displacement: body-agnostic primitives the
// player substrate and the agent controller both call, in the same order.
// See: context/lib/movement.md §6

use glam::Vec3;
use parry3d::shape::Capsule;

use crate::collision::moving::{
    CollisionSource, CombinedCastHit, CombinedCollisionWorld, MoverCollider, MoverPenetration,
    MoverPose, MoverPoseSource, deepest_mover_penetration, deepest_mover_push_penetration,
    deepest_mover_push_penetration_excluding_swept, point_inside_mover,
};
use crate::collision::{CollisionWorld, SKIN_DISTANCE, cast_capsule_parry};
use postretro_foundation::GroundRef;

pub(super) fn ground_ref_from_hit(hit: CombinedCastHit) -> GroundRef {
    match hit.source {
        CollisionSource::Static => GroundRef::World,
        CollisionSource::Mover(mover_id) => GroundRef::Mover(mover_id),
    }
}

pub(super) fn apply_mover_carry(
    position: Vec3,
    previous_ground: GroundRef,
    collision: &CombinedCollisionWorld<'_>,
) -> Vec3 {
    let GroundRef::Mover(mover_id) = previous_ground else {
        return position;
    };
    collision.poses.pose(mover_id).map_or(position, |pose| {
        let pivot = pose.transform.position - pose.tick_delta;
        pivot + pose.tick_rotation_delta * (position - pivot) + pose.tick_delta
    })
}

/// The velocity a body keeps from the mover it just left: `None` while it stays
/// on that mover, or when it rode none. Each controller decides where it lands.
pub(super) fn mover_release_velocity(
    previous_ground: GroundRef,
    ground: GroundRef,
    collision: &CombinedCollisionWorld<'_>,
    position: Vec3,
) -> Option<Vec3> {
    let GroundRef::Mover(mover_id) = previous_ground else {
        return None;
    };
    if ground == GroundRef::Mover(mover_id) {
        return None;
    }
    collision.poses.pose(mover_id).map(|pose| {
        let pivot = pose.transform.position - pose.tick_delta;
        pose.linear_velocity + pose.angular_velocity.cross(position - pivot)
    })
}

pub(super) fn displace_from_movers(
    previous_ground: GroundRef,
    collision: &CombinedCollisionWorld<'_>,
    capsule: &Capsule,
    position: Vec3,
) -> Vec3 {
    if collision.movers.is_empty() {
        return position;
    }
    let penetration = match previous_ground {
        GroundRef::Mover(mover_id) => deepest_mover_push_penetration_excluding_swept(
            collision.movers,
            collision.poses,
            position,
            capsule,
            mover_id,
        ),
        GroundRef::Airborne | GroundRef::World => {
            deepest_mover_push_penetration(collision.movers, collision.poses, position, capsule)
        }
    };
    let Some(penetration) = penetration else {
        return position;
    };
    let Some(mut candidate) =
        unblocked_mover_displacement(position, penetration, collision, capsule)
    else {
        return position;
    };

    // A rotating face can cross the capsule, then carry the first sweep
    // displacement into a different side of its final pose. Settle that
    // candidate with the same final-pose recovery pure rotators use.
    for _ in 0..4 {
        let Some(final_penetration) =
            deepest_mover_penetration(collision.movers, collision.poses, candidate, capsule)
        else {
            break;
        };
        let Some(recovered) =
            unblocked_mover_displacement(candidate, final_penetration, collision, capsule)
        else {
            return position;
        };
        candidate = recovered;
    }
    candidate
}

/// A body embedded in a mover: `Some` with where it ends up, `None` for any
/// other body, which [`displace_from_movers`] handles.
///
/// A body whose centre lies inside the hull: contact reads the nearest face
/// from inside, so its normal points deeper in, and left to
/// [`displace_from_movers`] a trailing face would drag the body along inside the
/// hull. This pushes it out through that face instead, or leaves it in place
/// when static geometry blocks the way. A body touching no face stays until the
/// mover moves off it.
///
/// A body already overlapping the mover before this tick's motion, its centre
/// outside: the overlap is no new hit, so the mover's sweep says nothing about
/// it, and its swept push would drag the body along behind a mover moving off
/// it. Present contact alone pushes it, again left in place when static
/// geometry blocks the way.
pub(super) fn displace_out_of_hull(
    previous_ground: GroundRef,
    collision: &CombinedCollisionWorld<'_>,
    capsule: &Capsule,
    position: Vec3,
) -> Option<Vec3> {
    let touching = deepest_mover_penetration(collision.movers, collision.poses, position, capsule)?;
    if previous_ground == GroundRef::Mover(touching.mover_id) {
        return None;
    }
    let mover = collision
        .movers
        .iter()
        .find(|mover| mover.mover_id == touching.mover_id)?;
    if !point_inside_mover(std::slice::from_ref(mover), collision.poses, position) {
        return embedded_before_tick(mover, collision.poses, capsule, position).then(|| {
            unblocked_mover_displacement(position, touching, collision, capsule).unwrap_or(position)
        });
    }
    // Contact depth is how far the capsule reaches past the face, plus one skin
    // of margin. Leaving through that face takes the capsule's whole extent
    // across it less that reach, and a skin more to end clear of it. A lift or
    // crusher face pierced end-on spans the capsule's axis as well as its radius.
    let reach = touching.depth - SKIN_DISTANCE;
    let depth = 2.0 * capsule_half_extent(capsule, touching.normal) - reach + SKIN_DISTANCE;
    if depth <= 0.0 {
        return Some(position);
    }
    let exit = MoverPenetration {
        mover_id: touching.mover_id,
        normal: -touching.normal,
        depth,
    };
    Some(unblocked_mover_displacement(position, exit, collision, capsule).unwrap_or(position))
}

/// Whether a capsule at `position` overlapped `mover` at its pose before this
/// tick's motion.
fn embedded_before_tick(
    mover: &MoverCollider,
    poses: &dyn MoverPoseSource,
    capsule: &Capsule,
    position: Vec3,
) -> bool {
    let Some(pose) = poses.pose(mover.mover_id) else {
        return false;
    };
    let mut start = pose;
    start.transform.position -= pose.tick_delta;
    start.transform.rotation = pose.tick_rotation_delta.inverse() * pose.transform.rotation;
    let at_start = TickStartPose {
        mover_id: mover.mover_id,
        pose: start,
    };
    let mover = std::slice::from_ref(mover);
    deepest_mover_penetration(mover, &at_start, position, capsule).is_some()
        || point_inside_mover(mover, &at_start, position)
}

/// One mover at its pose before this tick's motion.
struct TickStartPose {
    mover_id: u32,
    pose: MoverPose,
}

impl MoverPoseSource for TickStartPose {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        (mover_id == self.mover_id).then_some(self.pose)
    }
}

/// How far `capsule` extends from its centre along the unit `normal`.
fn capsule_half_extent(capsule: &Capsule, normal: Vec3) -> f32 {
    0.5 * (capsule.segment.b - capsule.segment.a).dot(normal).abs() + capsule.radius
}

/// Whether a capsule at `position` overlaps `mover_id` as it stands.
pub(super) fn embedded_in_mover(
    collision: &CombinedCollisionWorld<'_>,
    mover_id: u32,
    capsule: &Capsule,
    position: Vec3,
) -> bool {
    collision
        .movers
        .iter()
        .find(|mover| mover.mover_id == mover_id)
        .is_some_and(|mover| {
            deepest_mover_penetration(
                std::slice::from_ref(mover),
                collision.poses,
                position,
                capsule,
            )
            .is_some()
        })
}

fn unblocked_mover_displacement(
    position: Vec3,
    penetration: MoverPenetration,
    collision: &CombinedCollisionWorld<'_>,
    capsule: &Capsule,
) -> Option<Vec3> {
    let blocked =
        mover_push_is_blocked_by_static(collision.static_world, position, capsule, penetration);
    if blocked {
        #[cfg(debug_assertions)]
        log::warn!(
            "[Movement] mover {} push was blocked by static geometry; local displacement recovery left position unchanged",
            penetration.mover_id
        );
        None
    } else {
        Some(position + penetration.normal * penetration.depth)
    }
}

/// Whether the static world prevents the mover-overlap recovery push.
///
/// The host pin test (`kinematic_mover/pin.rs`) mirrors this skin cast, widened to
/// other movers; the predicted displacement path keeps its static-only test.
fn mover_push_is_blocked_by_static(
    static_world: &CollisionWorld,
    position: Vec3,
    capsule: &Capsule,
    penetration: MoverPenetration,
) -> bool {
    cast_capsule_parry(
        static_world,
        position,
        capsule,
        penetration.normal,
        penetration.depth,
    )
    .is_some()
}
