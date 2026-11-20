//! Shot and sight queries over static geometry plus kinematic movers at a pose.
//!
//! A mover contact is a world contact: callers resolve shots, splash and sight
//! the same way whichever surface stopped them. [`CombinedCastHit::source`]
//! is public for consumers that need the surface itself, such as cover.
// See: context/lib/networking.md §Combat authority · context/lib/entity_model.md §Components (Splash), §7c

use glam::Vec3;
use parry3d::math::Pose;
use parry3d::query::{RayCast, ShapeCastOptions, cast_shapes, intersection_test};
use parry3d::shape::Ball;

use super::moving::{
    CombinedCastHit, CombinedCollisionWorld, choose_nearest, mover_bound_may_reach_segment,
    mover_shape_hit, point_inside_posed_mover, static_shape_hit, transform_pose,
};
use super::{cast_sphere_exact_parry, inclusive_query_limit, within_limit};

/// Whether the exact segment from `eye` to `aim` is clear of static geometry
/// and of every mover at its pose. The static part is [`super::line_of_sight`]
/// exactly: a hit at the aim point counts as clear, and a degenerate segment
/// as blocked.
pub fn line_of_sight_combined(eye: Vec3, aim: Vec3, world: &CombinedCollisionWorld<'_>) -> bool {
    let to_aim = aim - eye;
    let distance = to_aim.length();
    if !distance.is_finite() || distance <= 1.0e-5 {
        return false;
    }

    let direction = to_aim / distance;
    let hit = super::moving::cast_ray_combined(
        world.static_world,
        world.movers,
        world.poses,
        eye,
        direction,
        distance,
    );
    !matches!(hit, Some(hit) if hit.time_of_impact < distance - 1.0e-4)
}

/// Sweep a sphere of its exact authored radius against static geometry and
/// every mover at its pose; the nearest contact wins. The static part is
/// [`super::cast_sphere_exact`] exactly. A zero radius sweeps a point, so it
/// returns the combined ray's hit from any origin off a surface; from an origin
/// on a surface it passes that surface, as the static sweep does.
///
/// A sphere that starts inside a mover's hull reports that hull's far face.
/// Ask [`mover_at_sphere`] first where an enclosed start must stop the sweep.
pub fn cast_sphere_exact_combined(
    world: &CombinedCollisionWorld<'_>,
    pos: Vec3,
    radius: f32,
    dir: Vec3,
    max_toi: f32,
) -> Option<CombinedCastHit> {
    let mut nearest = cast_sphere_exact_parry(world.static_world, pos, radius, dir, max_toi)
        .map(static_shape_hit);
    let ball = Ball::new(radius);
    let options = ShapeCastOptions {
        max_time_of_impact: inclusive_query_limit(max_toi),
        target_distance: 0.0,
        stop_at_penetration: false,
        ..Default::default()
    };
    let sphere_pose = Pose::from_translation(pos);
    for mover in world.movers {
        let Some(pose) = world.poses.pose(mover.mover_id) else {
            continue;
        };
        let mover_pose = transform_pose(pose.transform);
        if !mover_bound_may_reach_segment(mover, &mover_pose, pos, dir * max_toi, radius) {
            continue;
        }
        let hit = cast_shapes(
            &sphere_pose,
            dir,
            &ball,
            &mover_pose,
            Vec3::ZERO,
            &mover.local_mesh,
            options,
        )
        .ok()
        .flatten()
        .filter(|hit| within_limit(hit.time_of_impact, max_toi))
        .map(|hit| mover_shape_hit(hit, &mover_pose, mover.mover_id, pose));
        choose_nearest(&mut nearest, hit);
    }
    nearest
}

/// The first mover, in collider order, whose posed hull a sphere at `center`
/// touches or lies inside. A projectile in this state at the start of its step
/// is struck where it is: a sweep from inside a hull would report the far face.
pub fn mover_at_sphere(
    world: &CombinedCollisionWorld<'_>,
    center: Vec3,
    radius: f32,
) -> Option<u32> {
    if !center.is_finite() || !radius.is_finite() || radius < 0.0 {
        return None;
    }
    let ball = Ball::new(radius);
    let sphere_pose = Pose::from_translation(center);
    world.movers.iter().find_map(|mover| {
        let pose = world.poses.pose(mover.mover_id)?;
        let mover_pose = transform_pose(pose.transform);
        if !mover_bound_may_reach_segment(mover, &mover_pose, center, Vec3::ZERO, radius) {
            return None;
        }
        // Surface contact first: the enclosure test is undefined on the surface.
        let touches = intersection_test(&sphere_pose, &ball, &mover_pose, &mover.local_mesh)
            .is_ok_and(|hit| hit.intersecting);
        (touches || point_inside_posed_mover(mover, &mover_pose, center)).then_some(mover.mover_id)
    })
}

/// Distance along `dir` from `origin` to the first surface of mover
/// `mover_id`'s posed hull, within `max_toi`. From inside the hull this is
/// where the ray leaves it; from a point on the surface heading inward it is 0.
pub fn mover_surface_distance(
    world: &CombinedCollisionWorld<'_>,
    mover_id: u32,
    origin: Vec3,
    dir: Vec3,
    max_toi: f32,
) -> Option<f32> {
    let mover = world
        .movers
        .iter()
        .find(|mover| mover.mover_id == mover_id)?;
    let pose = world.poses.pose(mover_id)?;
    let ray = parry3d::query::Ray::new(origin, dir);
    mover
        .local_mesh
        .cast_ray(
            &transform_pose(pose.transform),
            &ray,
            inclusive_query_limit(max_toi),
            true,
        )
        .filter(|toi| within_limit(*toi, max_toi))
}

#[cfg(test)]
#[path = "shot_queries_tests.rs"]
mod tests;
