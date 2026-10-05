//! Skin-distance shape casts with a robust near-contact normal.
//!
//! Movement casts keep the capsule `SKIN_DISTANCE` from surfaces, so a resting
//! or sliding capsule starts every cast at the target-distance boundary, moving
//! tangent to the surface beneath it. parry's GJK-derived cast normal is
//! ill-conditioned there: a tangent slide across a large flat floor triangle
//! can report a hit a fraction of a millimetre out with a normal tilted by a
//! few degrees, and projecting velocity onto that normal bleeds speed and
//! corrupts the knockback bank. parry itself recognises the problem — for a
//! support-map pair it re-derives the normal from a closest-point contact and
//! drops casts moving away from the contact — but only below a `1e-4` TOI,
//! which is smaller than the grazing distances its tolerance (scaled with the
//! support-point magnitude, so with triangle size) produces.
//!
//! [`SkinCastDispatcher`] applies parry's fallback per sub-shape at the
//! engine's scale: any support-map hit reached within one skin width of travel
//! takes its normal and witnesses from a closest-point contact at the hit pose,
//! and a hit whose cast direction does not approach that contact is discarded,
//! as `stop_at_penetration: false` intends. A support-map leaf cast parry
//! misses is re-checked against the closest-point contact at the start pose:
//! starting inside the target-distance band and closing on the surface is a
//! hit at TOI 0 (parry 0.31.1 drops some of these in its trimesh traversal's
//! orientation, letting a capsule resting against a wall walk into it).
//! Composite shapes (trimeshes) recurse through this dispatcher, so a
//! discarded grazing triangle never masks a farther real hit.

use parry3d::math::{Pose, Real, Vector};
use parry3d::query::details::{
    cast_shapes_composite_shape_shape, cast_shapes_shape_composite_shape,
    contact_support_map_support_map,
};
use parry3d::query::{
    ClosestPoints, Contact, DefaultQueryDispatcher, NonlinearRigidMotion, QueryDispatcher,
    ShapeCastHit, ShapeCastOptions, ShapeCastStatus, ShapeDistance, ShapeIntersection, Unsupported,
};
use parry3d::shape::{Shape, SupportMap};

use super::SKIN_DISTANCE;

/// Hits reached within this much travel take the closest-point contact normal.
/// One skin width: the band in which a cast effectively starts at the
/// target-distance boundary.
const NEAR_CONTACT_TRAVEL: Real = SKIN_DISTANCE;

/// A near-contact hit counts as approaching only when the cast direction moves
/// into the contact normal by more than this cosine (about 0.06°). Below it the
/// motion is tangent to the surface and the sign of the dot product is solver
/// noise.
const MIN_APPROACH_COS: Real = 1.0e-3;

/// Shape-cast `g1` at `pos1` moving with `vel1` against `g2` at `pos2` moving
/// with `vel2`, with the skin-scale near-contact fallback. Same frames and
/// semantics as [`parry3d::query::cast_shapes`].
pub(crate) fn cast_shapes_skin(
    pos1: &Pose,
    vel1: Vector,
    g1: &dyn Shape,
    pos2: &Pose,
    vel2: Vector,
    g2: &dyn Shape,
    options: ShapeCastOptions,
) -> Option<ShapeCastHit> {
    let pos12 = pos1.inv_mul(pos2);
    let vel12 = pos1.rotation.inverse() * (vel2 - vel1);
    SkinCastDispatcher
        .cast_shapes(&pos12, vel12, g1, g2, options)
        .ok()
        .flatten()
}

/// Delegates every query to [`DefaultQueryDispatcher`] except `cast_shapes`,
/// which refines near-contact support-map hits (see the module docs).
struct SkinCastDispatcher;

impl SkinCastDispatcher {
    fn leaf_cast(
        pos12: &Pose,
        vel12: Vector,
        g1: &dyn Shape,
        g2: &dyn Shape,
        options: ShapeCastOptions,
    ) -> Result<Option<ShapeCastHit>, Unsupported> {
        let hit = DefaultQueryDispatcher.cast_shapes(pos12, vel12, g1, g2, options)?;
        let (Some(s1), Some(s2)) = (g1.as_support_map(), g2.as_support_map()) else {
            return Ok(hit);
        };
        Ok(match hit {
            Some(hit) => Self::refine_near_contact(pos12, vel12, s1, s2, options, hit),
            None => Self::missed_in_band_start(pos12, vel12, s1, s2, options),
        })
    }

    /// parry's contract for a cast that starts inside the target-distance band
    /// and closes on the surface is a hit at TOI 0. parry 0.31.1 misses some of
    /// these in the `(triangle, query shape)` orientation its trimesh traversal
    /// uses — a capsule resting against a wall then walks into it — so a miss
    /// is re-checked against the closest-point contact at the start pose.
    fn missed_in_band_start(
        pos12: &Pose,
        vel12: Vector,
        g1: &dyn SupportMap,
        g2: &dyn SupportMap,
        options: ShapeCastOptions,
    ) -> Option<ShapeCastHit> {
        if options.target_distance <= 0.0 {
            return None;
        }
        let contact = contact_support_map_support_map(pos12, g1, g2, options.target_distance)?;
        let approaching = contact.normal1.dot(vel12) < -MIN_APPROACH_COS * vel12.length();
        (contact.dist < options.target_distance && approaching).then_some(ShapeCastHit {
            time_of_impact: 0.0,
            witness1: contact.point1,
            witness2: contact.point2,
            normal1: contact.normal1,
            normal2: contact.normal2,
            status: ShapeCastStatus::PenetratingOrWithinTargetDist,
            subshape1: 0,
            subshape2: 0,
        })
    }

    fn refine_near_contact(
        pos12: &Pose,
        vel12: Vector,
        g1: &dyn SupportMap,
        g2: &dyn SupportMap,
        options: ShapeCastOptions,
        hit: ShapeCastHit,
    ) -> Option<ShapeCastHit> {
        let speed = vel12.length();
        let travel = hit.time_of_impact * speed;
        if travel.is_nan() || travel >= NEAR_CONTACT_TRAVEL {
            return Some(hit);
        }
        let at_hit = Pose::from_parts(
            pos12.translation + vel12 * hit.time_of_impact,
            pos12.rotation,
        );
        let Some(contact) = contact_support_map_support_map(&at_hit, g1, g2, Real::MAX) else {
            return Some(hit);
        };
        // `normal1` points from shape 1 toward shape 2 and `vel12` is shape 2's
        // velocity relative to shape 1, so approach is a negative dot product.
        let approaching = contact.normal1.dot(vel12) < -MIN_APPROACH_COS * speed;
        if !options.stop_at_penetration && !approaching {
            return None;
        }
        Some(ShapeCastHit {
            normal1: contact.normal1,
            normal2: contact.normal2,
            witness1: contact.point1,
            witness2: contact.point2,
            ..hit
        })
    }
}

impl QueryDispatcher for SkinCastDispatcher {
    fn intersection_test(
        &self,
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
    ) -> Result<ShapeIntersection, Unsupported> {
        DefaultQueryDispatcher.intersection_test(pos12, g1, g2)
    }

    fn distance(
        &self,
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
    ) -> Result<ShapeDistance, Unsupported> {
        DefaultQueryDispatcher.distance(pos12, g1, g2)
    }

    fn contact(
        &self,
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
        prediction: Real,
    ) -> Result<Option<Contact>, Unsupported> {
        DefaultQueryDispatcher.contact(pos12, g1, g2, prediction)
    }

    fn closest_points(
        &self,
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
        max_dist: Real,
    ) -> Result<ClosestPoints, Unsupported> {
        DefaultQueryDispatcher.closest_points(pos12, g1, g2, max_dist)
    }

    fn cast_shapes(
        &self,
        pos12: &Pose,
        local_vel12: Vector,
        g1: &dyn Shape,
        g2: &dyn Shape,
        options: ShapeCastOptions,
    ) -> Result<Option<ShapeCastHit>, Unsupported> {
        // Recurse through this dispatcher so each trimesh triangle is refined
        // individually.
        if let Some(c1) = g1.as_composite_shape() {
            return Ok(cast_shapes_composite_shape_shape(
                self,
                pos12,
                local_vel12,
                c1,
                g2,
                options,
            ));
        }
        if let Some(c2) = g2.as_composite_shape() {
            return Ok(cast_shapes_shape_composite_shape(
                self,
                pos12,
                local_vel12,
                g1,
                c2,
                options,
            ));
        }
        Self::leaf_cast(pos12, local_vel12, g1, g2, options)
    }

    fn cast_shapes_nonlinear(
        &self,
        motion1: &NonlinearRigidMotion,
        g1: &dyn Shape,
        motion2: &NonlinearRigidMotion,
        g2: &dyn Shape,
        start_time: Real,
        end_time: Real,
        stop_at_penetration: bool,
    ) -> Result<Option<ShapeCastHit>, Unsupported> {
        DefaultQueryDispatcher.cast_shapes_nonlinear(
            motion1,
            g1,
            motion2,
            g2,
            start_time,
            end_time,
            stop_at_penetration,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::{CollisionWorld, SKIN_DISTANCE};
    use parry3d::math::Vec3;
    use parry3d::shape::Capsule;

    /// Regression for a dash across a 40 m floor triangle: the capsule rests a
    /// few microns inside the skin band, and plain `cast_shapes` (parry 0.31.1)
    /// reports a hit ~2 mm out with a normal tilted ~2.8°, bleeding dash speed.
    #[test]
    fn tangent_slide_inside_the_skin_band_reports_no_tilted_floor_normal() {
        let world = CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(-20.0, 0.0, -20.0),
                Vec3::new(20.0, 0.0, -20.0),
                Vec3::new(20.0, 0.0, 20.0),
                Vec3::new(-20.0, 0.0, 20.0),
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        );
        let (half_height, radius) = (0.8, 0.4);
        let capsule = Capsule::new_y(half_height, radius);
        let rest_y = half_height + radius + SKIN_DISTANCE;
        let options = ShapeCastOptions {
            max_time_of_impact: 0.208,
            target_distance: SKIN_DISTANCE,
            stop_at_penetration: false,
            ..Default::default()
        };
        let start = Pose::from_translation(Vec3::new(0.00077, rest_y - 4.4e-6, 11.7066));
        let dir = Vec3::new(-1.2e-9, 0.0, 1.0);

        let skin = cast_shapes_skin(
            &start,
            dir,
            &capsule,
            &Pose::IDENTITY,
            Vec3::ZERO,
            &world.mesh,
            options,
        );

        assert!(
            skin.is_none(),
            "a tangent slide across the floor it rests on must not hit it: {skin:?}"
        );
    }

    #[test]
    fn near_contact_approach_still_hits_with_the_closest_point_normal() {
        let wall_x = 1.0;
        let world = CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(wall_x, -5.0, -5.0),
                Vec3::new(wall_x, 5.0, -5.0),
                Vec3::new(wall_x, 5.0, 5.0),
                Vec3::new(wall_x, -5.0, 5.0),
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        );
        let radius = 0.4;
        let capsule = Capsule::new_y(0.8, radius);
        let options = ShapeCastOptions {
            max_time_of_impact: 1.0,
            target_distance: SKIN_DISTANCE,
            stop_at_penetration: false,
            ..Default::default()
        };
        // 1 cm outside the skin band, closing at 45°: the hit lands within one
        // skin width of travel, so it takes the refined normal.
        let start =
            Pose::from_translation(Vec3::new(wall_x - radius - SKIN_DISTANCE - 0.01, 0.0, 0.0));
        let dir = Vec3::new(1.0, 0.0, 1.0).normalize();

        let hit = cast_shapes_skin(
            &start,
            dir,
            &capsule,
            &Pose::IDENTITY,
            Vec3::ZERO,
            &world.mesh,
            options,
        )
        .expect("closing on the wall must hit it");

        let expected_toi = 0.01 * std::f32::consts::SQRT_2;
        assert!(
            (hit.time_of_impact - expected_toi).abs() < 1.0e-4,
            "expected TOI ≈ {expected_toi}, got {}",
            hit.time_of_impact
        );
        assert!(
            (hit.normal2 - Vec3::NEG_X).length() < 1.0e-4,
            "expected the wall's closest-point normal, got {:?}",
            hit.normal2
        );
    }

    /// Regression for an agent walking into a wall it already rests against:
    /// parry 0.31.1's `(triangle, capsule)` leaf cast (the orientation its
    /// trimesh traversal uses) misses the wall when the cast starts inside the
    /// skin band, so the floor graze was the only candidate and the agent
    /// stepped 4.7 cm into the wall.
    #[test]
    fn cast_starting_inside_a_walls_skin_band_hits_the_wall() {
        let world = CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(-50.0, 0.0, -50.0),
                Vec3::new(50.0, 0.0, -50.0),
                Vec3::new(50.0, 0.0, 50.0),
                Vec3::new(-50.0, 0.0, 50.0),
                Vec3::new(2.0, 0.0, -50.0),
                Vec3::new(2.0, 4.0, -50.0),
                Vec3::new(2.0, 4.0, 50.0),
                Vec3::new(2.0, 0.0, 50.0),
            ],
            vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]],
        );
        let capsule = Capsule::new_y(0.55, 0.35);
        let options = ShapeCastOptions {
            max_time_of_impact: 0.083_333_63,
            target_distance: SKIN_DISTANCE,
            stop_at_penetration: false,
            ..Default::default()
        };
        // Surface 1.98 cm from the wall and a hair inside the floor's band.
        let start = Pose::from_translation(Vec3::new(1.630_153, 0.919_958_7, 2.337_401_6));
        let dir = Vec3::new(4.000_025, -0.000_264_167_8, 2.999_996).normalize();

        let hit = cast_shapes_skin(
            &start,
            dir,
            &capsule,
            &Pose::IDENTITY,
            Vec3::ZERO,
            &world.mesh,
            options,
        )
        .expect("closing on the wall from inside its skin band must hit it");

        assert!(
            hit.time_of_impact < 1.0e-4,
            "expected an immediate hit, got {hit:?}"
        );
        assert!(
            (hit.normal2 - Vec3::NEG_X).length() < 1.0e-3,
            "expected the wall normal, got {:?}",
            hit.normal2
        );
    }
}
