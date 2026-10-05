//! Exact closest points between a capsule and a triangle.
//!
//! parry answers this pair through GJK, whose f32 termination tolerance
//! scales with the support-point magnitude. Beside the edge of a 40 m
//! triangle its closest-point distance can be off by over 2 cm and its normal
//! by several degrees, with the error depending on argument order. The skin
//! cast measures sub-millimetre gaps against a 2 cm skin, so it computes this
//! pair directly. When the capsule's core segment does not reach the
//! triangle, their closest points lie at a segment endpoint against the
//! triangle face, or on the segment against a triangle edge (Ericson,
//! *Real-Time Collision Detection*, §5.1). The candidates use parry's exact
//! point-triangle projection and segment-segment closest points.

use parry3d::math::{Pose, Vector};
use parry3d::query::details::closest_points_segment_segment_with_locations_nD;
use parry3d::query::{Contact, PointQuery};
use parry3d::shape::{Capsule, Segment, Shape, Triangle};

/// Contact between `g1` and `g2` in `contact_support_map_support_map`'s
/// frames, when the pair is a capsule and a triangle (either order) and the
/// capsule's core segment stays clear of the triangle. `None` otherwise; a
/// core that reaches the triangle is a deep penetration that parry's EPA
/// answers.
pub(crate) fn contact(pos12: &Pose, g1: &dyn Shape, g2: &dyn Shape) -> Option<Contact> {
    if let (Some(capsule), Some(triangle)) = (g1.as_capsule(), g2.as_triangle()) {
        return capsule_triangle(pos12, capsule, triangle);
    }
    if let (Some(triangle), Some(capsule)) = (g1.as_triangle(), g2.as_capsule()) {
        return capsule_triangle(&pos12.inverse(), capsule, triangle).map(Contact::flipped);
    }
    None
}

/// `pos12` places the triangle in the capsule's frame.
fn capsule_triangle(pos12: &Pose, capsule: &Capsule, triangle: &Triangle) -> Option<Contact> {
    let triangle_1 = triangle.transformed(pos12);
    let (on_segment, on_triangle) =
        closest_points_segment_triangle(capsule.segment.a, capsule.segment.b, &triangle_1)?;
    let offset = on_triangle - on_segment;
    let core_distance = offset.length();
    // Below this the core touches the triangle and the normal is undefined.
    if core_distance <= 1.0e-5 {
        return None;
    }
    let normal1 = offset / core_distance;
    Some(Contact::new(
        on_segment + normal1 * capsule.radius,
        pos12.inverse_transform_point(on_triangle),
        normal1,
        pos12.rotation.inverse() * -normal1,
        core_distance - capsule.radius,
    ))
}

/// Closest points `(on segment, on triangle)` between segment `pq` and
/// `triangle`. A segment that crosses the triangle yields coincident points.
/// `None` for a degenerate triangle, which falls back to parry's GJK.
fn closest_points_segment_triangle(
    p: Vector,
    q: Vector,
    triangle: &Triangle,
) -> Option<(Vector, Vector)> {
    let normal = triangle.normal()?;
    let mut best = (p, triangle.project_local_point(p, true).point);
    let mut best_distance = best.0.distance_squared(best.1);
    let mut consider = |on_segment: Vector, on_triangle: Vector| {
        let distance = on_segment.distance_squared(on_triangle);
        if distance < best_distance {
            best = (on_segment, on_triangle);
            best_distance = distance;
        }
    };

    consider(q, triangle.project_local_point(q, true).point);
    let pq = Segment::new(p, q);
    for edge in triangle.edges() {
        let (on_pq, on_edge) =
            closest_points_segment_segment_with_locations_nD((&p, &q), (&edge.a, &edge.b));
        consider(pq.point_at(&on_pq), edge.point_at(&on_edge));
    }
    // A segment crossing the triangle touches it at the plane crossing. A
    // crossing outside the triangle is just another valid pair, never closer
    // than the true minimum.
    let (dp, dq) = ((p - triangle.a).dot(normal), (q - triangle.a).dot(normal));
    if dp * dq < 0.0 {
        let crossing = p + (q - p) * (dp / (dp - dq));
        consider(crossing, triangle.project_local_point(crossing, true).point);
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parry3d::math::{Real, Vec3};

    const EPS: Real = 1.0e-5;

    fn big_floor_triangle() -> Triangle {
        Triangle::new(
            Vec3::new(-20.0, 0.0, -20.0),
            Vec3::new(20.0, 0.0, -20.0),
            Vec3::new(20.0, 0.0, 20.0),
        )
    }

    /// Beside the diagonal edge of a 40 m triangle, where parry's GJK misses
    /// the gap by centimetres, both argument orders match the analytic answer.
    #[test]
    fn capsule_beside_a_large_triangle_edge_matches_the_analytic_gap() {
        let capsule = Capsule::new_y(0.8, 0.4);
        let triangle = big_floor_triangle();
        // Bottom sphere centre 0.42 m above the floor plane and 0.0969 m
        // outside the triangle's x = z edge.
        let center = Vec3::new(0.696, 1.22, 0.833);
        let offset = (0.833_f32 - 0.696) / 2.0_f32.sqrt();
        let expected_dist = (0.42_f32 * 0.42 + offset * offset).sqrt() - 0.4;
        let pose = Pose::from_translation(center);

        let capsule_first = contact(&pose.inverse(), &capsule, &triangle).expect("clear pair");
        let triangle_first = contact(&pose, &triangle, &capsule).expect("clear pair");

        assert!((capsule_first.dist - expected_dist).abs() < EPS);
        assert!((triangle_first.dist - expected_dist).abs() < EPS);
        let toward_edge = Vec3::new(1.0, 0.0, -1.0).normalize();
        let expected_normal1 = (toward_edge * offset - Vec3::Y * 0.42).normalize();
        assert!((capsule_first.normal1 - expected_normal1).length() < EPS);
        assert!((triangle_first.normal2 - expected_normal1).length() < EPS);
        assert!((triangle_first.normal1 + expected_normal1).length() < EPS);
        // Witnesses are local to their own shape.
        assert!(
            (capsule_first.point2 - Vec3::new(0.7645, 0.0, 0.7645)).length() < 1.0e-3,
            "{:?}",
            capsule_first.point2
        );
        assert!((triangle_first.point1 - capsule_first.point2).length() < EPS);
        assert!((triangle_first.point2 - capsule_first.point1).length() < EPS);
    }

    #[test]
    fn capsule_above_a_triangle_face_measures_from_its_bottom_sphere() {
        let capsule = Capsule::new_y(0.8, 0.4);
        let pose = Pose::from_translation(Vec3::new(3.0, 1.22, -7.0));

        let c = contact(&pose.inverse(), &capsule, &big_floor_triangle()).expect("clear pair");

        assert!((c.dist - 0.02).abs() < EPS, "{c:?}");
        assert!((c.normal1 - Vec3::NEG_Y).length() < EPS);
        assert!((c.point1 - Vec3::new(0.0, -1.2, 0.0)).length() < EPS);
    }

    #[test]
    fn lying_capsule_beside_a_wall_edge_uses_the_segment_against_the_edge() {
        // Capsule core along X at height 1, clear of the wall triangle's
        // vertical edge at x = 2, z = 0: the closest feature pair is the
        // segment's end against that edge.
        let capsule = Capsule::new_x(0.5, 0.25);
        let wall = Triangle::new(
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(2.0, 4.0, 0.0),
            Vec3::new(2.0, 0.0, 5.0),
        );
        let pose = Pose::from_translation(Vec3::new(1.0, 1.0, -0.3));

        let c = contact(&pose.inverse(), &capsule, &wall).expect("clear pair");

        // Segment end (1.5, 1, -0.3) to edge point (2, 1, 0).
        let expected = (0.5_f32 * 0.5 + 0.3 * 0.3).sqrt() - 0.25;
        assert!((c.dist - expected).abs() < EPS, "{c:?}");
    }

    #[test]
    fn a_core_that_crosses_the_triangle_defers_to_parry() {
        let capsule = Capsule::new_y(0.8, 0.4);
        let pose = Pose::from_translation(Vec3::new(3.0, 0.1, -7.0));

        assert!(contact(&pose.inverse(), &capsule, &big_floor_triangle()).is_none());
    }

    #[test]
    fn other_shape_pairs_are_not_handled() {
        let ball = parry3d::shape::Ball::new(0.5);
        assert!(contact(&Pose::IDENTITY, &ball, &big_floor_triangle()).is_none());
    }
}
