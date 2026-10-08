//! Skin-distance shape casts that honour the skin-band contract.
//!
//! Movement casts keep the capsule `SKIN_DISTANCE` from surfaces, so a resting
//! or sliding capsule starts every cast at the target-distance boundary, often
//! moving tangent to the surface beneath it. [`SkinCastDispatcher`] uses parry
//! only to choose candidate triangles (its BVH traversal); it answers each
//! capsule–triangle leaf itself, by conservative advancement on exact
//! closest-point contacts.
//!
//! # Why not parry's leaf cast
//!
//! Background for why the engine does not trust parry 0.31's GJK cast at the
//! band boundary:
//!
//! - It can report a hit for a slide that never closes, or a hit short of the
//!   band, with a normal tilted by up to tens of degrees on large triangles;
//!   projecting velocity onto it bleeds speed and corrupts the knockback bank.
//!   parry 0.30.2 (the fix for dimforge/parry#429, PR #430) removed the
//!   extent-scaled slack that left resting capsules a few millimetres out, so
//!   every slide now starts in this ill-conditioned case. parry's own remedy —
//!   re-derive the normal from a contact, drop a cast that does not close —
//!   only runs below a fixed `1e-4` TOI.
//! - It can miss a closing cast that starts at the boundary (open issue
//!   dimforge/parry#452), so a capsule resting against a wall walks into it.
//! - It can place the hit of a cast that starts inside the band well past
//!   TOI 0, letting a shallow creep reach the wall.
//! - Its closest points drift by centimetres beside the edges of large
//!   triangles (see `capsule_triangle`).
//!
//! # Conservative advancement
//!
//! Each leaf steps forward from the cast's start, as Bullet's and Box2D's
//! time-of-impact solvers do. The signed gap `d(t)` between convex shapes
//! under translation is convex in `t`, penetration included, and
//! `rate = normal1 · vel12` is its exact derivative (`normal1` points from
//! shape 1 toward shape 2; `vel12` is shape 2's velocity relative to
//! shape 1). A convex function never falls below its tangent line, so:
//!
//! - `rate >= 0`: the gap never shrinks again — no hit (parry's rule).
//! - Gap above the band: the Newton step `t += gap / -rate` stays at or
//!   before the first band crossing (up to f32 noise in the gap), so the first
//!   pose that reaches the band is that crossing, and a step past
//!   `max_time_of_impact` is a miss.
//! - At the band, or starting inside it: the hit there is kept only if the
//!   gap falls below `target - MAX_BAND_INTRUSION` within the rest of the
//!   cast. The same Newton steps continue toward that level: one that passes
//!   `max_time_of_impact`, or a `rate >= 0`, proves the gap stays above it and
//!   drops the hit; reaching it keeps the hit. A dropped hit therefore never
//!   lets the cast end more than [`MAX_BAND_INTRUSION`] inside the band, and a
//!   graze that only touches the band (for example the far edge of a coplanar
//!   neighbour) is dropped even though its tangent line dips below the level.
//! - Steps run out ([`MAX_ADVANCE_STEPS`]) with the gap still closing on its
//!   level inside the cast: the answer is the conservative one, a hit at the
//!   band pose or, if the band was never reached, at the last pose, which is
//!   still short of it. A cast that starts outside the band advances on its
//!   first step, so it never gets a TOI-0 hit.
//!
//! Normal and witnesses come from the contact at the returned pose. Starting
//! from the cast's start rather than from parry's TOI makes the leaf answer
//! independent of parry's: a warm start from that TOI had to detect and
//! restart from overshoots, and f32 staircase noise in the gap at kilometre
//! coordinates made the detection misfire into TOI-0 hits.
//!
//! The intrusion allowance is an absolute bound, not a per-cast one. A cast
//! that starts inside the band keeps its hit as soon as its motion would end
//! deeper than the bound, so a shallow creep stops there instead of
//! accumulating, as it does under a cosine slack on the closing test. Without
//! the allowance, float noise in the contact normal makes a tangent slide that
//! starts a hair inside the band look like a closing one.
//!
//! Composite shapes (trimeshes) recurse through this dispatcher, so each
//! triangle is advanced on its own and a dropped grazing triangle never masks
//! a farther real hit. parry's traversal keeps the nearest leaf TOI and prunes
//! BVH nodes the swept bound reaches no earlier; a leaf TOI is never later
//! than its first band crossing, which lies inside its node's bound, so the
//! pruning stays sound. Pairs with no exact contact — not capsule–triangle, a
//! degenerate triangle, or a capsule core piercing the triangle at a visited
//! pose — fall back to parry's cast for that leaf.
//!
//! # Known limitation: coplanar seams
//!
//! A slide across the shared edge of two coplanar triangles approaches the
//! neighbour through that edge, so the neighbour's gap only grazes the band
//! and its contact normal is tilted toward the edge. From a start within the
//! intrusion allowance the closing test sees the gap level off and drops the
//! hit, but nothing guarantees that under f32 noise. From a start `depth`
//! deeper inside the band than the allowance, the slide hits at TOI 0 with a
//! normal tilted by up to about
//! `sqrt(2 * (depth + BAND_REACHED_TOLERANCE) / (radius + target))` — 4° from
//! 1 mm deep for the player capsule. The proper fix is internal-edge handling
//! (treating a shared coplanar edge as part of the face), an owner follow-up.
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
};
use parry3d::query::{
    ClosestPoints, Contact, DefaultQueryDispatcher, NonlinearRigidMotion, QueryDispatcher,
    ShapeCastHit, ShapeCastOptions, ShapeCastStatus, ShapeDistance, ShapeIntersection, Unsupported,
};
use parry3d::shape::Shape;

use super::capsule_triangle;

/// A closest-point gap within this much of `target_distance` counts as having
/// reached the band, and likewise for the intrusion allowance's level: 0.1%
/// of `SKIN_DISTANCE`, several f32 ulps at 100 m coordinates. A cast that only
/// grazes the band settles where its gap is this small, which leaves its
/// normal tilted by about
/// `sqrt(2 * tolerance / (radius + target))` — 0.56° for the 0.4 m player
/// capsule.
const BAND_REACHED_TOLERANCE: Real = 2.0e-5;

/// How far inside the target-distance band the rest of a cast may carry the
/// shapes before a closing hit counts. 2.5% of `SKIN_DISTANCE`, so a resting
/// capsule always keeps at least 19.5 mm of the 20 mm skin — a positional
/// slop in the sense of Box2D's `linearSlop` or PhysX's rest offset. It
/// absorbs a contact-normal error of up to `MAX_BAND_INTRUSION / travel` rad
/// — `1.5e-3` for a 0.33 m dash tick, far above f32 noise.
const MAX_BAND_INTRUSION: Real = 5.0e-4;

/// Contacts evaluated per leaf before the advancement stops with a
/// conservative hit. A face approach reaches the band in one step and its
/// closing test takes one more; an edge or vertex approach takes a few. A
/// tangential graze halves its distance to the touch point per step once
/// within about one capsule radius of it, so it reaches
/// [`BAND_REACHED_TOLERANCE`] within about nine contacts from any distance,
/// and its closing test needs one or two more. Eight let almost half of seam
/// crossings from up to 1 m away run out and keep a tilted hit.
const MAX_ADVANCE_STEPS: usize = 12;

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

/// Delegates every query to [`DefaultQueryDispatcher`] except `cast_shapes`,
/// which advances capsule–triangle leaves onto the band (see the module docs).
struct SkinCastDispatcher;

impl SkinCastDispatcher {
    fn leaf_cast(
        pos12: &Pose,
        vel12: Vector,
        g1: &dyn Shape,
        g2: &dyn Shape,
        options: ShapeCastOptions,
    ) -> Result<Option<ShapeCastHit>, Unsupported> {
        match Self::advance(pos12, vel12, g1, g2, options) {
            Some(hit) => Ok(hit),
            None => DefaultQueryDispatcher.cast_shapes(pos12, vel12, g1, g2, options),
        }
    }

    /// Conservative advancement from the cast's start (see the module docs).
    /// The outer `None` means some visited pose has no exact contact, and the
    /// caller falls back to parry's cast.
    fn advance(
        pos12: &Pose,
        vel12: Vector,
        g1: &dyn Shape,
        g2: &dyn Shape,
        options: ShapeCastOptions,
    ) -> Option<Option<ShapeCastHit>> {
        let target = options.target_distance;
        let mut toi: Real = 0.0;
        // The hit at the first pose that reached the band, once found.
        let mut band_hit = None;
        let mut steps = 0;
        loop {
            steps += 1;
            let at = Pose::from_parts(pos12.translation + vel12 * toi, pos12.rotation);
            let contact = capsule_triangle::contact(&at, g1, g2)?;
            let rate = contact.normal1.dot(vel12);
            if rate >= 0.0 {
                // The gap never shrinks again: the band is never reached, or
                // the gap stays above the intrusion allowance.
                return Some(None);
            }
            if band_hit.is_none() && contact.dist - target <= BAND_REACHED_TOLERANCE {
                band_hit = Some(shape_cast_hit(toi, &contact, target, true));
            }
            // Advance to the band, then on toward the allowance to decide
            // whether the band hit counts.
            let level = match band_hit {
                Some(_) => target - MAX_BAND_INTRUSION,
                None => target,
            };
            let gap = contact.dist - level;
            if band_hit.is_some() && gap <= BAND_REACHED_TOLERANCE {
                return Some(band_hit);
            }
            let next = toi + gap / -rate;
            if next > options.max_time_of_impact {
                return Some(None);
            }
            if steps == MAX_ADVANCE_STEPS {
                // Still closing on the level within the cast: stop at the
                // band, or short of it if the band was never reached.
                return Some(Some(
                    band_hit.unwrap_or_else(|| shape_cast_hit(toi, &contact, target, false)),
                ));
            }
            toi = next;
        }
    }
}

/// A hit at `toi` from the contact there. `converged` is false for a pose
/// short of the band, taken when the steps ran out.
fn shape_cast_hit(toi: Real, contact: &Contact, target: Real, converged: bool) -> ShapeCastHit {
    ShapeCastHit {
        time_of_impact: toi,
        witness1: contact.point1,
        witness2: contact.point2,
        normal1: contact.normal1,
        normal2: contact.normal2,
        status: if !converged {
            ShapeCastStatus::OutOfIterations
        } else if contact.dist < target {
            ShapeCastStatus::PenetratingOrWithinTargetDist
        } else {
            ShapeCastStatus::Converged
        },
        subshape1: 0,
        subshape2: 0,
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
        // Recurse through this dispatcher so each trimesh triangle is advanced
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

    /// Deterministic xorshift sampler for the sweep tests.
    struct TestRng(u64);

    impl TestRng {
        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            let unit = (self.0 >> 40) as f32 / (1u64 << 24) as f32;
            lo + (hi - lo) * unit
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
            "a tangent slide within the floor triangle it rests on must not hit it: {skin:?}"
        );
    }

    /// The same failure far past the first skin width of travel: parry 0.31.1
    /// reports a spurious floor hit 11.6 cm into the slide with a normal
    /// tilted 3.5°. Advancement is valid at any TOI, so there is no hit.
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
            "a long tangent slide within one floor triangle must not hit it: {skin:?}"
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
    /// 53°, and parry 0.31.1's leaf cast drops the wall (dimforge/parry#452).
    /// The floor graze was the only remaining candidate and the agent stepped
    /// 4.7 cm into the wall.
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

    /// Regression: at 1 km coordinates the f32 gap moves in steps of about
    /// 6e-5, coarser than the band tolerance. Warm-started from parry's TOI,
    /// the settle loop read a step landing a hair inside the band as parry
    /// overshooting and restarted from the cast's start until its steps ran
    /// out, stopping ~1% of wall approaches at TOI 0 — up to 15 cm short —
    /// with the start pose's normal.
    #[test]
    fn wall_approach_at_kilometre_coordinates_hits_at_the_band() {
        let (origin, extent) = (1000.0, 1000.0);
        let wall_x = origin + 2.0;
        let world = CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(origin - extent, origin, origin - extent),
                Vec3::new(origin + extent, origin, origin - extent),
                Vec3::new(origin + extent, origin, origin + extent),
                Vec3::new(origin - extent, origin, origin + extent),
                Vec3::new(wall_x, origin - 1.0, origin - extent),
                Vec3::new(wall_x, origin + 10.0, origin - extent),
                Vec3::new(wall_x, origin + 10.0, origin + extent),
                Vec3::new(wall_x, origin - 1.0, origin + extent),
            ],
            vec![[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7]],
        );
        let (half_height, radius) = (0.8, 0.4);
        let capsule = Capsule::new_y(half_height, radius);
        let travel = 0.5;
        let mut rng = TestRng(0xdead_beef);
        let mut checked = 0;

        for _ in 0..400 {
            // Resting in the floor's band, outside the wall's, closing on it.
            let start_gap = rng.range(0.0205, 0.3);
            let heading = rng.range(3.0, 89.0).to_radians();
            let dir = Vec3::new(heading.sin(), 0.0, heading.cos());
            let start = Vec3::new(
                wall_x - radius - start_gap,
                origin + half_height + radius + SKIN_DISTANCE - rng.range(0.0, 5.0e-6),
                origin + rng.range(-0.8 * extent, 0.8 * extent),
            );
            let expected_toi = (start_gap - SKIN_DISTANCE) / heading.sin();
            if expected_toi > travel - 0.02 {
                // Too close to the end of travel to require a hit.
                continue;
            }
            checked += 1;

            let hit = cast_shapes_skin(
                &Pose::from_translation(start),
                dir,
                &capsule,
                &Pose::IDENTITY,
                Vec3::ZERO,
                &world.mesh,
                skin_options(travel),
            )
            .unwrap_or_else(|| panic!("closing on the wall from {start:?} must hit it"));

            // f32 spacing at 1 km is 6.1e-5 m; the gap at the hit stays
            // within a few spacings of the band.
            let gap_error = (hit.time_of_impact - expected_toi) * heading.sin();
            assert!(
                gap_error.abs() < 2.0e-4,
                "hit {gap_error} m off the band from {start:?} along {dir:?}: {hit:?}"
            );
            assert!(
                hit.normal2.angle_between(Vec3::NEG_X) < 1.0_f32.to_radians(),
                "expected the wall normal from {start:?}, got {:?}",
                hit.normal2
            );
        }
        assert!(checked > 300, "only {checked} casts required a hit");
    }

    /// A capsule whose core pierces the triangle has no closest-point normal,
    /// so the leaf defers to parry's cast rather than guess one.
    #[test]
    fn cast_from_a_piercing_start_defers_to_parry() {
        let triangle = parry3d::shape::Triangle::new(
            Vec3::new(-50.0, 0.0, -50.0),
            Vec3::new(50.0, 0.0, -50.0),
            Vec3::new(50.0, 0.0, 50.0),
        );
        let capsule = Capsule::new_y(0.8, 0.4);
        let start = Pose::from_translation(Vec3::new(20.0, 0.3, -10.0));
        let options = skin_options(0.2);

        let skin = cast_shapes_skin(
            &start,
            Vec3::NEG_Y,
            &capsule,
            &Pose::IDENTITY,
            Vec3::ZERO,
            &triangle,
            options,
        );
        let plain = cast_shapes(
            &start,
            Vec3::NEG_Y,
            &capsule,
            &Pose::IDENTITY,
            Vec3::ZERO,
            &triangle,
            options,
        )
        .unwrap();

        assert_eq!(
            skin.map(|hit| (hit.time_of_impact, hit.normal1, hit.normal2)),
            plain.map(|hit| (hit.time_of_impact, hit.normal1, hit.normal2)),
        );
    }

    /// Known limitation until internal-edge handling: a slide across the
    /// shared edge of two coplanar triangles can hit the triangle it slides
    /// onto, whose closest feature on approach is that edge. The hit's normal
    /// is tilted by at most about
    /// `sqrt(2 * (depth + BAND_REACHED_TOLERANCE) / (radius + SKIN_DISTANCE))`
    /// for a start `depth` inside the band: 0.56° at the band, 4° from 1 mm.
    /// A start within the intrusion allowance hits rarely, because the closing
    /// test follows the graze down to the allowance rather than trusting its
    /// tangent line: warm-started from parry's TOI with a tangent-line test,
    /// 55% of such slides hit. Approaches of up to 1 m also exercise the step
    /// budget.
    #[test]
    fn slide_across_a_coplanar_seam_hits_rarely_and_with_a_bounded_tilt() {
        let world = large_floor();
        let (half_height, radius) = (0.8, 0.4);
        let capsule = Capsule::new_y(half_height, radius);
        let seam = Vec3::new(1.0, 0.0, 1.0).normalize();
        let across = Vec3::new(1.0, 0.0, -1.0).normalize();
        let mut rng = TestRng(42);

        for depth in [0.0, 1.0e-4, 1.0e-3] {
            let tilt_bound =
                (2.0 * (depth + BAND_REACHED_TOLERANCE) / (radius + SKIN_DISTANCE)).sqrt();
            let (casts, mut hits) = (400, 0);
            for _ in 0..casts {
                let heading = rng.range(5.0, 175.0).to_radians();
                let dir = seam * heading.cos() + across * heading.sin();
                let p = seam * rng.range(-10.0, 10.0) - dir * rng.range(0.0, 1.0);
                let start = Vec3::new(p.x, half_height + radius + SKIN_DISTANCE - depth, p.z);
                let Some(hit) = cast_shapes_skin(
                    &Pose::from_translation(start),
                    dir,
                    &capsule,
                    &Pose::IDENTITY,
                    Vec3::ZERO,
                    &world.mesh,
                    skin_options(1.2),
                ) else {
                    continue;
                };
                hits += 1;
                let tilt = hit.normal2.angle_between(Vec3::Y);
                assert!(
                    tilt <= tilt_bound * 1.05,
                    "seam hit {depth} m inside the band tilted {}°, bound {}°",
                    tilt.to_degrees(),
                    tilt_bound.to_degrees()
                );
            }
            if depth < MAX_BAND_INTRUSION {
                assert!(
                    hits * 100 <= casts,
                    "{hits}/{casts} seam crossings {depth} m inside the band hit"
                );
            }
        }
    }
}
