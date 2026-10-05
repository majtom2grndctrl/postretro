//! Skin-distance shape casts that honour the skin-band contract.
//!
//! Movement casts keep the capsule `SKIN_DISTANCE` from surfaces, so a resting
//! or sliding capsule starts every cast at the target-distance boundary, often
//! moving tangent to the surface beneath it. parry 0.31's casts misbehave at
//! that boundary:
//!
//! - **Spurious and misplaced hits.** GJK's directional distance can report a
//!   hit for a slide that never closes on the surface, or a hit short of the
//!   band, with a normal tilted by up to tens of degrees on large triangles;
//!   projecting velocity onto it bleeds speed and corrupts the knockback bank.
//!   This happens at every triangle size and in parry 0.17 too. What changed
//!   is that parry 0.30.2 (the fix for dimforge/parry#429, PR #430) removed
//!   the extent-scaled slack that left short-TOI hits a few millimetres out,
//!   so a capsule now rests exactly at the boundary and every slide starts in
//!   the ill-conditioned case. parry's own remedy — re-derive the normal from
//!   a closest-point contact and drop a cast that does not close — only runs
//!   below a fixed `1e-4` TOI.
//! - **Missed in-band starts.** A cast that starts at the boundary (gap equal
//!   to `target_distance` within f32 rounding) and closes on the surface can
//!   return no hit, in either argument order: open issue dimforge/parry#452
//!   (an absolute `eps_tol` in `minkowski_ray_cast`). A capsule resting
//!   against a wall then walks into it.
//! - **Late hits.** A cast that starts inside the band can report its hit well
//!   past TOI 0, letting a shallow creep reach the wall.
//!
//! [`SkinCastDispatcher`] therefore settles every support-map leaf cast onto
//! the band itself:
//!
//! - It starts from parry's TOI, or from the cast's start when parry missed
//!   within [`IN_BAND_START_TOLERANCE`] of the band or placed its hit inside
//!   the band, and steps forward with closest-point contacts until the gap
//!   reaches the band. The gap between convex shapes under translation is
//!   convex in time, so Newton steps from the left never pass the first
//!   crossing. That holds at any TOI, so there is no travel gate.
//! - At the band it keeps the hit only when the rest of the cast would carry
//!   the shapes more than [`MAX_BAND_INTRUSION`] inside it, and never when the
//!   motion does not close (parry's rule, `normal1 · vel12 >= 0`).
//! - Normal and witnesses come from the closest-point contact at the settled
//!   pose. Every skin cast reduces to capsule–triangle leaves, for which that
//!   contact is computed exactly (`capsule_triangle`): parry's GJK misjudges
//!   it by centimetres beside the edges of large triangles.
//!
//! The intrusion allowance is an absolute bound, not a per-cast one. A cast
//! that starts inside the band keeps its hit as soon as its motion would end
//! deeper than the bound, so a shallow creep stops there instead of
//! accumulating, as it does under a cosine slack on the closing test. Without
//! the allowance, float noise in the contact normal makes a tangent slide that
//! starts a hair inside the band look like a closing one.
//!
//! Composite shapes (trimeshes) recurse through this dispatcher, so each
//! triangle is settled on its own and a dropped grazing triangle never masks a
//! farther real hit.
//!
//! A custom `QueryDispatcher` is parry's extension point for per-pair query
//! behaviour. The alternative — accept parry's hit, then nudge off the surface
//! and depenetrate after the move, as Rapier's character controller does —
//! trades the exact band for nudge distances and iteration counts tuned
//! against feel. Fixing the query keeps the contract movement and mover
//! callers rely on: a cast never ends more than [`MAX_BAND_INTRUSION`] inside
//! the `SKIN_DISTANCE` band.

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

use super::capsule_triangle;

/// How far outside `target_distance` a missed cast may start and still be
/// re-checked as an in-band start. dimforge/parry#452 drops casts whose start
/// gap equals `target_distance` to within f32 rounding; measured contact
/// distances for those misses reach `0.0202` against a `0.02` target. With
/// `5e-4` this dispatcher missed none of 29,400 closing casts per triangle
/// size in a sweep (start gaps 0.2–20 mm, angles 2.5–87.5°, triangles 2–100
/// m). It is 2.5% of `SKIN_DISTANCE`, and the re-check settles the hit onto
/// the band, so it never stops a capsule early.
const IN_BAND_START_TOLERANCE: Real = 5.0e-4;

/// A closest-point gap within this much of `target_distance` counts as having
/// reached the band: 0.1% of `SKIN_DISTANCE`, several f32 ulps at 100 m
/// coordinates. A cast that only grazes the band (sliding past a coplanar
/// neighbour triangle's edge) settles where its gap is this small, which
/// leaves its normal tilted by about `sqrt(2 * tolerance / radius)` — 0.6° for
/// the 0.4 m player capsule.
const BAND_REACHED_TOLERANCE: Real = 2.0e-5;

/// How far inside the target-distance band the rest of a cast may carry the
/// shapes before a closing hit counts. 2.5% of `SKIN_DISTANCE`, so a resting
/// capsule always keeps at least 19.5 mm of the 20 mm skin. It absorbs a
/// contact-normal error of up to `MAX_BAND_INTRUSION / travel` rad — `1.5e-3`
/// for a 0.33 m dash tick, far above f32 noise.
const MAX_BAND_INTRUSION: Real = 5.0e-4;

/// Newton steps toward the band before the current (conservative) pose is
/// taken as the hit. A transversal approach converges in one or two; a graze
/// that only touches the band halves its distance to the touch point per
/// step, so five bring a 10 cm graze within [`BAND_REACHED_TOLERANCE`].
const MAX_SETTLE_STEPS: usize = 8;

/// Shape-cast `g1` at `pos1` moving with `vel1` against `g2` at `pos2` moving
/// with `vel2`, with the skin-band fixes. Same frames and semantics as
/// [`parry3d::query::cast_shapes`]: witnesses and normals are local to their
/// shape. Assumes `stop_at_penetration: false`, as every skin cast uses.
pub(crate) fn cast_shapes_skin(
    pos1: &Pose,
    vel1: Vector,
    g1: &dyn Shape,
    pos2: &Pose,
    vel2: Vector,
    g2: &dyn Shape,
    options: ShapeCastOptions,
) -> Option<ShapeCastHit> {
    debug_assert!(!options.stop_at_penetration);
    let pos12 = pos1.inv_mul(pos2);
    let vel12 = pos1.rotation.inverse() * (vel2 - vel1);
    SkinCastDispatcher
        .cast_shapes(&pos12, vel12, g1, g2, options)
        .ok()
        .flatten()
}

/// A support-map leaf pair. Closest-point contacts come from the exact
/// capsule–triangle query when the pair is one (every skin cast's leaves
/// are), and from parry's GJK otherwise.
#[derive(Clone, Copy)]
struct LeafPair<'a> {
    g1: &'a dyn Shape,
    g2: &'a dyn Shape,
    s1: &'a dyn SupportMap,
    s2: &'a dyn SupportMap,
}

impl LeafPair<'_> {
    /// Closest-point contact at `pos12` within `prediction`, in the frames
    /// `contact_support_map_support_map` uses.
    fn contact(&self, pos12: &Pose, prediction: Real) -> Option<Contact> {
        match capsule_triangle::contact(pos12, self.g1, self.g2) {
            Some(contact) => (contact.dist <= prediction).then_some(contact),
            None => contact_support_map_support_map(pos12, self.s1, self.s2, prediction),
        }
    }
}

/// No contact at an unbounded prediction: GJK failed, so the caller keeps
/// parry's own answer.
struct ContactFailed;

/// Delegates every query to [`DefaultQueryDispatcher`] except `cast_shapes`,
/// which settles support-map leaf casts onto the band (see the module docs).
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
        let pair = LeafPair { g1, g2, s1, s2 };
        Ok(match hit {
            Some(hit) if hit.time_of_impact.is_finite() => {
                Self::settle(pos12, vel12, pair, options, hit.time_of_impact).unwrap_or(Some(hit))
            }
            Some(hit) => Some(hit),
            None => Self::missed_in_band_start(pos12, vel12, pair, options),
        })
    }

    /// parry's contract for a cast that starts inside the target-distance band
    /// and closes on the surface is a hit at TOI 0; dimforge/parry#452 misses
    /// some of these at the band boundary, so a miss that starts within
    /// [`IN_BAND_START_TOLERANCE`] of the band is settled from TOI 0.
    fn missed_in_band_start(
        pos12: &Pose,
        vel12: Vector,
        pair: LeafPair<'_>,
        options: ShapeCastOptions,
    ) -> Option<ShapeCastHit> {
        if options.target_distance <= 0.0 {
            return None;
        }
        let band = options.target_distance + IN_BAND_START_TOLERANCE;
        pair.contact(pos12, band)?;
        Self::settle(pos12, vel12, pair, options, 0.0).unwrap_or(None)
    }

    /// Walk a cast forward from `toi` to where it reaches the band, then keep
    /// it as a hit only if the rest of the cast closes past
    /// [`MAX_BAND_INTRUSION`]. Normal and witnesses come from the
    /// closest-point contact at the returned pose. A `toi` already inside the
    /// band is past the first crossing, so the walk restarts from the cast's
    /// start.
    ///
    /// The gap `d(t)` between convex shapes under translation is convex, so
    /// it never falls below its tangent line `d + rate * Δt`, where
    /// `rate = normal1 · vel12` (`normal1` points from shape 1 toward shape 2
    /// and `vel12` is shape 2's velocity relative to shape 1). Hence:
    ///
    /// - `rate >= 0`: the gap never shrinks again — no hit (parry's rule).
    /// - gap above the band: the band is not reached before the tangent
    ///   line's crossing, so stepping there is safe (Newton's method from the
    ///   left on a convex function never overshoots the first crossing). This
    ///   corrects GJK hits placed short of the band, which grazing casts past
    ///   a triangle edge produce with normals tilted by up to ~30°.
    /// - at the band: the gap stays above `target - MAX_BAND_INTRUSION` for
    ///   the rest of the cast unless the tangent line drops below it, so a hit
    ///   is dropped exactly when keeping it cannot matter by more than the
    ///   allowance.
    fn settle(
        pos12: &Pose,
        vel12: Vector,
        pair: LeafPair<'_>,
        options: ShapeCastOptions,
        mut toi: Real,
    ) -> Result<Option<ShapeCastHit>, ContactFailed> {
        let target = options.target_distance;
        let mut steps = 0;
        loop {
            let at = Pose::from_parts(pos12.translation + vel12 * toi, pos12.rotation);
            let contact = pair.contact(&at, Real::MAX).ok_or(ContactFailed)?;
            let gap = contact.dist - target;
            steps += 1;
            if gap < -BAND_REACHED_TOLERANCE && toi > 0.0 {
                // parry placed the hit past the first crossing (seen for casts
                // that start inside the band); settle from the start instead.
                toi = 0.0;
                continue;
            }
            let rate = contact.normal1.dot(vel12);
            if rate >= 0.0 {
                return Ok(None);
            }
            if gap > BAND_REACHED_TOLERANCE && steps < MAX_SETTLE_STEPS {
                toi += gap / -rate;
                if toi > options.max_time_of_impact {
                    return Ok(None);
                }
                continue;
            }
            let remaining = options.max_time_of_impact - toi;
            let closes_past_band = contact.dist + rate * remaining < target - MAX_BAND_INTRUSION;
            return Ok(closes_past_band.then_some(ShapeCastHit {
                time_of_impact: toi,
                witness1: contact.point1,
                witness2: contact.point2,
                normal1: contact.normal1,
                normal2: contact.normal2,
                status: if contact.dist < target {
                    ShapeCastStatus::PenetratingOrWithinTargetDist
                } else {
                    ShapeCastStatus::Converged
                },
                subshape1: 0,
                subshape2: 0,
            }));
        }
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
        // Recurse through this dispatcher so each trimesh triangle is settled
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
    use parry3d::query::cast_shapes;
    use parry3d::shape::Capsule;

    /// The 40 m floor quad the dash regressions were found on.
    fn large_floor() -> CollisionWorld {
        CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(-20.0, 0.0, -20.0),
                Vec3::new(20.0, 0.0, -20.0),
                Vec3::new(20.0, 0.0, 20.0),
                Vec3::new(-20.0, 0.0, 20.0),
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        )
    }

    fn skin_options(max_time_of_impact: f32) -> ShapeCastOptions {
        ShapeCastOptions {
            max_time_of_impact,
            target_distance: SKIN_DISTANCE,
            stop_at_penetration: false,
            ..Default::default()
        }
    }

    /// Regression for a dash across a 40 m floor triangle: the capsule rests a
    /// few microns inside the skin band, and plain `cast_shapes` (parry 0.31.1)
    /// reports a hit ~2 mm out with a normal tilted ~2.8°, bleeding dash speed.
    #[test]
    fn tangent_slide_inside_the_skin_band_reports_no_tilted_floor_normal() {
        let world = large_floor();
        let (half_height, radius) = (0.8, 0.4);
        let capsule = Capsule::new_y(half_height, radius);
        let rest_y = half_height + radius + SKIN_DISTANCE;
        let start = Pose::from_translation(Vec3::new(0.00077, rest_y - 4.4e-6, 11.7066));
        let dir = Vec3::new(-1.2e-9, 0.0, 1.0);

        let skin = cast_shapes_skin(
            &start,
            dir,
            &capsule,
            &Pose::IDENTITY,
            Vec3::ZERO,
            &world.mesh,
            skin_options(0.208),
        );

        assert!(
            skin.is_none(),
            "a tangent slide across the floor it rests on must not hit it: {skin:?}"
        );
    }

    /// The same failure far past the first skin width of travel: parry 0.31.1
    /// reports a spurious floor hit 11.6 cm into the slide with a normal
    /// tilted 3.5°. Settling is valid at any TOI, so it is dropped too.
    #[test]
    fn long_tangent_slide_reports_no_tilted_floor_normal_beyond_one_skin_width() {
        let world = large_floor();
        let (half_height, radius) = (0.8, 0.4);
        let capsule = Capsule::new_y(half_height, radius);
        let rest_y = half_height + radius + SKIN_DISTANCE;
        let start = Pose::from_translation(Vec3::new(-15.0, rest_y - 2.0e-6, -2.88));
        let heading = 187.0_f32.to_radians();
        let dir = Vec3::new(heading.cos(), 0.0, heading.sin());
        let options = skin_options(0.5);

        // Precondition: the fixture still exercises a spurious long-travel hit.
        // If a parry upgrade stops producing one, find a new fixture.
        let plain = cast_shapes(
            &start,
            dir,
            &capsule,
            &Pose::IDENTITY,
            Vec3::ZERO,
            &world.mesh,
            options,
        )
        .unwrap()
        .expect("fixture: parry reports a spurious floor hit");
        assert!(
            plain.time_of_impact > 5.0 * SKIN_DISTANCE
                && plain.normal2.angle_between(Vec3::Y) > 1.0_f32.to_radians(),
            "fixture: expected a tilted hit well past one skin width, got {plain:?}"
        );

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
            "a long tangent slide across the floor must not hit it: {skin:?}"
        );
    }

    /// A pawn spawned 1 cm inside the band of a 1 km floor, deeper than the
    /// intrusion allowance, gets no slack from it: any closing rate keeps a
    /// hit. Its level slide must still read as tangent, or every slide
    /// iteration would stop at TOI 0 and pin it in place.
    #[test]
    fn level_slide_from_deep_inside_the_band_does_not_hit() {
        let world = CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(-500.0, 0.0, -500.0),
                Vec3::new(500.0, 0.0, -500.0),
                Vec3::new(500.0, 0.0, 500.0),
                Vec3::new(-500.0, 0.0, 500.0),
            ],
            vec![[0, 2, 1], [0, 3, 2]],
        );
        let capsule = Capsule::new_y(0.8, 0.4);
        let start = Pose::from_translation(Vec3::new(0.0, 1.21, 0.0));

        let skin = cast_shapes_skin(
            &start,
            Vec3::NEG_Z,
            &capsule,
            &Pose::IDENTITY,
            Vec3::ZERO,
            &world.mesh,
            skin_options(0.03),
        );

        assert!(
            skin.is_none(),
            "a level slide must not hit the floor it starts inside: {skin:?}"
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
        // 1 cm outside the skin band, closing at 45°.
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
            skin_options(1.0),
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
    /// the capsule starts 0.15 mm inside the wall's skin band, closing at about
    /// 53°, and parry 0.31.1 drops the wall in the `(triangle, capsule)` order
    /// a trimesh traversal happens to use (dimforge/parry#452 — either order
    /// misses such boundary starts at similar rates). The floor graze was the
    /// only remaining candidate and the agent stepped 4.7 cm into the wall.
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
            skin_options(0.083_333_63),
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

    /// A capsule creeping into a long wall at a shallow angle, slid along it
    /// the way the movement loop does, never ends more than
    /// [`MAX_BAND_INTRUSION`] inside the skin band. A cosine slack on the
    /// closing test let a 0.05° creep reach 8.7 mm; parry's late hits for
    /// casts starting in the band let steeper ones reach the wall itself.
    #[test]
    fn shallow_wall_creep_stays_within_the_band_intrusion_bound() {
        let wall_x = 2.0;
        let world = CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(wall_x, -5.0, -100.0),
                Vec3::new(wall_x, 5.0, -100.0),
                Vec3::new(wall_x, 5.0, 100.0),
                Vec3::new(wall_x, -5.0, 100.0),
            ],
            vec![[0, 1, 2], [0, 2, 3], [0, 2, 1], [0, 3, 2]],
        );
        let radius = 0.35;
        let capsule = Capsule::new_y(0.55, radius);
        let tick_travel = 0.1667;
        let intrusion = |p: Vec3| SKIN_DISTANCE - (wall_x - (p.x + radius));

        for degrees in [0.005_f32, 0.05, 0.5, 2.0, 10.0, 45.0] {
            let heading = degrees.to_radians();
            let dir = Vec3::new(heading.sin(), 0.0, heading.cos());
            let mut p = Vec3::new(wall_x - radius - SKIN_DISTANCE, 0.0, -95.0);
            let mut worst = intrusion(p);
            for _ in 0..1100 {
                let (mut slide_dir, mut remaining) = (dir, tick_travel);
                for _ in 0..3 {
                    let hit = cast_shapes_skin(
                        &Pose::from_translation(p),
                        slide_dir,
                        &capsule,
                        &Pose::IDENTITY,
                        Vec3::ZERO,
                        &world.mesh,
                        skin_options(remaining),
                    );
                    let Some(hit) = hit else {
                        p += slide_dir * remaining;
                        break;
                    };
                    p += slide_dir * hit.time_of_impact;
                    remaining -= hit.time_of_impact;
                    slide_dir =
                        (slide_dir - hit.normal2 * slide_dir.dot(hit.normal2)).normalize_or_zero();
                    worst = worst.max(intrusion(p));
                }
                worst = worst.max(intrusion(p));
            }
            assert!(
                worst <= MAX_BAND_INTRUSION + 1.0e-5,
                "a {degrees}° creep intruded {worst} m into the skin band"
            );
        }
    }
}
