// Where a pinned agent can step to: the nearest standing point the mover's
// course never reaches. Runs on the pin edge alone, so its casts stay off the
// per-tick path.
// See: context/lib/movement.md §6

use std::f32::consts::TAU;

use glam::{Quat, Vec3};
use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry, Transform};
use postretro_foundation::GroundRef;

use super::pin::OneMoverPose;
use crate::collision::moving::{
    CombinedCollisionWorld, EMPTY_MOVER_POSES, MoverCollider, MoverPose,
    cast_capsule_combined_parry, deepest_mover_penetration, point_inside_mover,
};
use crate::collision::{COS_WALKABLE, CollisionCapsule, SKIN_DISTANCE};
use crate::movement::swept::floor_below;
use postretro_entities::KinematicMoverComponent;

/// Candidates per ring. Sixteen leaves no gap wider than a capsule at the
/// distances a pinned body can reach in the time a mover waits on it.
const RING_ANGLES: u32 = 16;

/// Spacing of the mover poses sampled along its course. Below the narrowest
/// capsule, so a capsule cannot sit between two samples unseen.
const COURSE_SAMPLE_SPACING: f32 = 0.25;

/// Rotations sampled per course point for a mover that spins.
const SPIN_SAMPLES: u32 = 12;

/// How squarely a face must oppose the walk to stop it, as a cosine.
const WALK_BLOCKING_FACE: f32 = 0.2;

/// Fractions of the walk checked against the pinning mover's present pose.
const WALK_SAMPLES: [f32; 2] = [0.5, 0.75];

/// The nearest point within `reach` where a capsule stands on a floor, walks to
/// in a straight line, and stays clear of every pose of `mover_id`'s course.
/// `None` when no such point exists, as in a pit the mover fills. Ring order is
/// fixed, so two runs pick the same point.
pub fn find_mover_clear_point(
    registry: &EntityRegistry,
    collision: &CombinedCollisionWorld<'_>,
    mover_id: u32,
    position: Vec3,
    capsule: CollisionCapsule,
    step_height: f32,
    reach: f32,
) -> Option<Vec3> {
    let collider = collision
        .movers
        .iter()
        .find(|collider| collider.mover_id() == mover_id)?;
    let (mover, transform) = mover_by_id(registry, mover_id)?;
    let course = MoverCourse {
        mover,
        collider,
        transform,
    };
    let spacing = capsule.radius.max(COURSE_SAMPLE_SPACING);
    let rings = (reach / spacing).floor() as u32;
    for ring in 1..=rings {
        let distance = spacing * ring as f32;
        for step in 0..RING_ANGLES {
            let angle = TAU * step as f32 / RING_ANGLES as f32;
            let offset = Vec3::new(angle.cos(), 0.0, angle.sin()) * distance;
            let Some(point) = standing_point(collision, capsule, position + offset, step_height)
            else {
                continue;
            };
            if walk_is_clear(collision, collider, capsule, position, point, step_height)
                && !course.reaches(point, capsule)
            {
                return Some(point);
            }
        }
    }
    None
}

fn mover_by_id(
    registry: &EntityRegistry,
    mover_id: u32,
) -> Option<(&KinematicMoverComponent, Transform)> {
    registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .find_map(|(entity, value)| match value {
            ComponentValue::KinematicMover(mover) if mover.mover_id == mover_id => registry
                .get_component::<Transform>(entity)
                .ok()
                .map(|transform| (mover, *transform)),
            _ => None,
        })
}

/// The capsule pose resting on a walkable floor near `candidate`, clear of
/// static geometry and of every mover as they stand now.
fn standing_point(
    collision: &CombinedCollisionWorld<'_>,
    capsule: CollisionCapsule,
    candidate: Vec3,
    step_height: f32,
) -> Option<Vec3> {
    let lifted = candidate + Vec3::Y * step_height;
    let floor = floor_below(
        collision,
        capsule,
        lifted,
        2.0 * step_height + SKIN_DISTANCE,
        COS_WALKABLE,
    )?;
    let point = lifted - Vec3::Y * floor.drop;
    let parry = capsule.parry();
    let clear = !collision.static_world.overlaps_static(point, capsule)
        && deepest_mover_penetration(collision.movers, collision.poses, point, &parry).is_none()
        && !point_inside_mover(collision.movers, collision.poses, point);
    (clear && !matches!(floor.ground, GroundRef::Airborne)).then_some(point)
}

/// Whether the body can walk straight to `point`: no wall in the way, at floor
/// height or a step up, and no passing through the pinning mover.
fn walk_is_clear(
    collision: &CombinedCollisionWorld<'_>,
    collider: &MoverCollider,
    capsule: CollisionCapsule,
    position: Vec3,
    point: Vec3,
    step_height: f32,
) -> bool {
    let parry = capsule.parry();
    let walk = Vec3::new(point.x - position.x, 0.0, point.z - position.z);
    let distance = walk.length();
    if distance <= f32::EPSILON {
        return false;
    }
    let direction = walk / distance;
    // A wall the body slides along is no obstacle, and a pinned body is often
    // pressed against one: only a face turned against the walk blocks it.
    let blocked_from = |start: Vec3| {
        cast_capsule_combined_parry(
            collision.static_world,
            &[],
            &EMPTY_MOVER_POSES,
            start,
            &parry,
            direction,
            distance,
        )
        .is_some_and(|hit| hit.normal.dot(direction) < -WALK_BLOCKING_FACE)
    };
    if blocked_from(position) && blocked_from(position + Vec3::Y * step_height) {
        return false;
    }
    // A body can start embedded, so the walk may keep that depth but not add to it.
    let pinning = std::slice::from_ref(collider);
    let depth_at = |at: Vec3| {
        deepest_mover_penetration(pinning, collision.poses, at, &parry)
            .map_or(0.0, |penetration| penetration.depth)
    };
    let allowed = depth_at(position) + 2.0 * SKIN_DISTANCE;
    WALK_SAMPLES
        .iter()
        .all(|fraction| depth_at(position + walk * *fraction) <= allowed)
}

/// Every pose a mover's path can put it in, its present pose included.
struct MoverCourse<'a> {
    mover: &'a KinematicMoverComponent,
    collider: &'a MoverCollider,
    transform: Transform,
}

impl MoverCourse<'_> {
    fn reaches(&self, point: Vec3, capsule: CollisionCapsule) -> bool {
        if self.reaches_at(self.transform.position, point, capsule) {
            return true;
        }
        let waypoints = &self.mover.waypoints;
        if let [only] = waypoints.as_slice() {
            return self.reaches_at(*only, point, capsule);
        }
        waypoints.windows(2).any(|leg| {
            let span = leg[1] - leg[0];
            let samples = (span.length() / COURSE_SAMPLE_SPACING).ceil().max(1.0) as u32;
            (0..=samples).any(|sample| {
                let at = leg[0] + span * (sample as f32 / samples as f32);
                self.reaches_at(at, point, capsule)
            })
        })
    }

    fn reaches_at(&self, mover_at: Vec3, point: Vec3, capsule: CollisionCapsule) -> bool {
        let spins = self.mover.spin_axis != Vec3::ZERO
            && (self.mover.spin_rate_rad_s != 0.0 || self.mover.spin_target_rate_rad_s != 0.0);
        let turns = if spins { SPIN_SAMPLES } else { 1 };
        (0..turns).any(|turn| {
            let spin = if spins {
                Quat::from_axis_angle(self.mover.spin_axis, TAU * turn as f32 / turns as f32)
            } else {
                Quat::IDENTITY
            };
            self.overlaps(mover_at, spin * self.transform.rotation, point, capsule)
        })
    }

    fn overlaps(
        &self,
        mover_at: Vec3,
        rotation: Quat,
        point: Vec3,
        capsule: CollisionCapsule,
    ) -> bool {
        let pose = OneMoverPose {
            mover_id: self.mover.mover_id,
            pose: MoverPose {
                transform: Transform {
                    position: mover_at,
                    rotation,
                    ..self.transform
                },
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::ZERO,
                tick_rotation_delta: Quat::IDENTITY,
                carry_yaw: false,
                tick_dt: 0.0,
            },
        };
        let collider = std::slice::from_ref(self.collider);
        deepest_mover_penetration(collider, &pose, point, &capsule.parry()).is_some()
            || point_inside_mover(collider, &pose, point)
    }
}
