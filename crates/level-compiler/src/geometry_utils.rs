// Shared polygon clipping utilities (Sutherland-Hodgman).
// See: context/lib/build_pipeline.md §PRL Compilation

use glam::DVec3;

/// Split a convex polygon by a plane using Sutherland-Hodgman clipping.
///
/// Returns `(front, back)` where front is the portion on the positive side
/// of the plane (dot(v, normal) - distance > 0) and back is the negative side.
/// Either may be `None` if the split produces a degenerate polygon (< 3 vertices).
///
/// `epsilon` controls point classification tolerance. Callers use different
/// values depending on context: BSP building uses a generous epsilon (0.1),
/// portal clipping uses a tighter one (0.01) to avoid accumulating error
/// across many sequential clips.
pub fn split_polygon(
    vertices: &[DVec3],
    plane_normal: DVec3,
    plane_distance: f64,
    epsilon: f64,
) -> (Option<Vec<DVec3>>, Option<Vec<DVec3>>) {
    let mut front_verts = Vec::new();
    let mut back_verts = Vec::new();

    let n = vertices.len();
    for i in 0..n {
        let current = vertices[i];
        let next = vertices[(i + 1) % n];
        let d_current = current.dot(plane_normal) - plane_distance;
        let d_next = next.dot(plane_normal) - plane_distance;

        let current_front = d_current > epsilon;
        let current_back = d_current < -epsilon;
        let current_on = !current_front && !current_back;

        let next_front = d_next > epsilon;
        let next_back = d_next < -epsilon;

        if current_front {
            front_verts.push(current);
        } else if current_back {
            back_verts.push(current);
        } else {
            // On the plane — belongs to both sides.
            front_verts.push(current);
            back_verts.push(current);
        }

        // Edge crosses the plane — compute intersection.
        let crosses = (current_front && next_back) || (current_back && next_front);
        if crosses {
            let t = d_current / (d_current - d_next);
            let intersection = current + t * (next - current);
            front_verts.push(intersection);
            back_verts.push(intersection);
        }
        // When current is on-plane, we already emitted it to both sides.
        // We still need to check if next is on the opposite side from the
        // previous non-on vertex — but the standard Sutherland-Hodgman loop
        // handles this naturally since on-plane points don't trigger a crossing.
        // The edge from on-plane to front/back doesn't need an intersection
        // because the on-plane point is already shared.
        let _ = current_on; // suppress unused warning; kept for clarity
    }

    let front = if front_verts.len() >= 3 {
        Some(front_verts)
    } else {
        None
    };

    let back = if back_verts.len() >= 3 {
        Some(back_verts)
    } else {
        None
    };

    (front, back)
}

/// Clip a convex polygon to the front (positive) side of a plane.
///
/// Returns `None` if the polygon is entirely behind the plane or the result
/// is degenerate (< 3 vertices).
pub fn clip_polygon_to_front(
    vertices: &[DVec3],
    plane_normal: DVec3,
    plane_distance: f64,
    epsilon: f64,
) -> Option<Vec<DVec3>> {
    split_polygon(vertices, plane_normal, plane_distance, epsilon).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_polygon_bisects_quad() {
        let verts = vec![
            DVec3::new(-2.0, 0.0, 0.0),
            DVec3::new(2.0, 0.0, 0.0),
            DVec3::new(2.0, 2.0, 0.0),
            DVec3::new(-2.0, 2.0, 0.0),
        ];

        let (front, back) = split_polygon(&verts, DVec3::X, 0.0, 0.1);
        let front = front.expect("front should exist");
        let back = back.expect("back should exist");

        assert!(front.len() >= 3);
        assert!(back.len() >= 3);

        // All front vertices on positive side or on plane
        for v in &front {
            assert!(v.x >= -0.1, "front vertex x={} behind plane", v.x);
        }
        // All back vertices on negative side or on plane
        for v in &back {
            assert!(v.x <= 0.1, "back vertex x={} in front of plane", v.x);
        }
    }

    #[test]
    fn split_polygon_entirely_front_returns_none_back() {
        let verts = vec![
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(2.0, 0.0, 0.0),
            DVec3::new(2.0, 1.0, 0.0),
        ];

        let (front, back) = split_polygon(&verts, DVec3::X, 0.0, 0.1);
        assert!(front.is_some());
        assert!(back.is_none());
    }

    #[test]
    fn split_polygon_entirely_back_returns_none_front() {
        let verts = vec![
            DVec3::new(-2.0, 0.0, 0.0),
            DVec3::new(-1.0, 0.0, 0.0),
            DVec3::new(-1.0, 1.0, 0.0),
        ];

        let (front, back) = split_polygon(&verts, DVec3::X, 0.0, 0.1);
        assert!(front.is_none());
        assert!(back.is_some());
    }

    #[test]
    fn clip_to_front_returns_front_half() {
        let verts = vec![
            DVec3::new(-2.0, 0.0, 0.0),
            DVec3::new(2.0, 0.0, 0.0),
            DVec3::new(2.0, 2.0, 0.0),
            DVec3::new(-2.0, 2.0, 0.0),
        ];

        let front = clip_polygon_to_front(&verts, DVec3::X, 0.0, 0.1);
        assert!(front.is_some());
        let front = front.unwrap();
        for v in &front {
            assert!(v.x >= -0.1);
        }
    }
}
