// Mover carry, release velocity, and displacement for player movement.
// See: context/lib/movement.md §6

use glam::Vec3;
use parry3d::math::{Point, Vector};
use parry3d::shape::Capsule;

use crate::collision::cast_capsule;
use crate::collision::moving::{
    CollisionSource, CombinedCastHit, CombinedCollisionWorld, MoverPenetration,
    deepest_mover_penetration, deepest_mover_push_penetration,
    deepest_mover_push_penetration_excluding_swept,
};
use postretro_foundation::{GroundRef, PlayerMovementComponent};

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

pub(super) fn apply_mover_release_velocity(
    component: &mut PlayerMovementComponent,
    previous_ground: GroundRef,
    collision: &CombinedCollisionWorld<'_>,
    player_position: Vec3,
) {
    let GroundRef::Mover(mover_id) = previous_ground else {
        return;
    };
    if component.ground == GroundRef::Mover(mover_id) {
        return;
    }
    if let Some(pose) = collision.poses.pose(mover_id) {
        let pivot = pose.transform.position - pose.tick_delta;
        component.velocity +=
            pose.linear_velocity + pose.angular_velocity.cross(player_position - pivot);
    }
}

pub(super) fn displace_from_movers(
    _component: &PlayerMovementComponent,
    previous_ground: GroundRef,
    collision: &CombinedCollisionWorld<'_>,
    capsule: &Capsule,
    position: Vec3,
) -> Vec3 {
    if collision.movers.is_empty() {
        return position;
    }
    let pos = Point::new(position.x, position.y, position.z);
    let penetration = match previous_ground {
        GroundRef::Mover(mover_id) => deepest_mover_push_penetration_excluding_swept(
            collision.movers,
            collision.poses,
            pos,
            capsule,
            mover_id,
        ),
        GroundRef::Airborne | GroundRef::World => {
            deepest_mover_push_penetration(collision.movers, collision.poses, pos, capsule)
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
        let candidate_point = Point::new(candidate.x, candidate.y, candidate.z);
        let Some(final_penetration) =
            deepest_mover_penetration(collision.movers, collision.poses, candidate_point, capsule)
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

fn unblocked_mover_displacement(
    position: Vec3,
    penetration: MoverPenetration,
    collision: &CombinedCollisionWorld<'_>,
    capsule: &Capsule,
) -> Option<Vec3> {
    let blocked = cast_capsule(
        collision.static_world,
        Point::new(position.x, position.y, position.z),
        capsule,
        Vector::new(
            penetration.normal.x,
            penetration.normal.y,
            penetration.normal.z,
        ),
        penetration.depth,
    )
    .is_some();
    if blocked {
        #[cfg(debug_assertions)]
        log::warn!(
            "[Movement] mover {} push was blocked by static geometry; pinch/crush resolution is deferred",
            penetration.mover_id
        );
        None
    } else {
        Some(position + penetration.normal * penetration.depth)
    }
}
