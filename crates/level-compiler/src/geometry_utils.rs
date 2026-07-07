// Shared polygon clipping utilities (Sutherland-Hodgman).
// See: context/lib/build_pipeline.md §PRL Compilation

use glam::DVec3;

/// Initial plane-winding half-extent. Covers any reasonable level geometry.
const WINDING_HALF_EXTENT: f64 = 16384.0;

/// Split a convex polygon by a plane using Sutherland-Hodgman clipping.
///
/// Returns `(front, back)` where front is the portion on the positive side
/// of the plane (dot(v, normal) - distance > 0) and back is the negative side.
/// Either may be `None` if the split produces a degenerate polygon (< 3 vertices).
///
/// `epsilon` controls point classification tolerance. Callers choose it by
/// operation: BSP face extraction uses `0.1`, portal clipping uses `0.01`,
/// and exact region-polytope clipping uses `1e-6`.
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

/// Build a large bounded polygon on an arbitrary plane.
pub(crate) fn make_base_winding(normal: DVec3, distance: f64) -> Vec<DVec3> {
    // Pick a reference axis not near-parallel to the normal to form a stable basis.
    let reference = if normal.z.abs() > 0.9 {
        DVec3::X
    } else {
        DVec3::Z
    };

    let basis1 = normal.cross(reference).normalize();
    let basis2 = normal.cross(basis1).normalize();

    let center = normal * distance;
    let half = WINDING_HALF_EXTENT;

    // CCW when viewed from the front (positive normal side).
    vec![
        center - basis1 * half - basis2 * half,
        center + basis1 * half - basis2 * half,
        center + basis1 * half + basis2 * half,
        center - basis1 * half + basis2 * half,
    ]
}

/// Clip a winding through a list of half-spaces.
pub(crate) fn clip_winding_to_half_spaces(
    mut winding: Vec<DVec3>,
    planes: &[(DVec3, f64)],
    epsilon: f64,
) -> Option<Vec<DVec3>> {
    for &(normal, distance) in planes {
        winding = clip_polygon_to_front(&winding, normal, distance, epsilon)?;
    }

    Some(winding)
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

    #[test]
    fn base_winding_lies_on_plane() {
        let normal = DVec3::Y;
        let distance = 5.0;
        let winding = make_base_winding(normal, distance);

        assert_eq!(winding.len(), 4);
        for v in &winding {
            let d = v.dot(normal) - distance;
            assert!(d.abs() < 1e-4, "winding vertex {v} not on plane (d={d})");
        }
    }

    #[test]
    fn base_winding_non_degenerate_for_axis_aligned_normals() {
        for normal in [
            DVec3::X,
            DVec3::Y,
            DVec3::Z,
            DVec3::NEG_X,
            DVec3::NEG_Y,
            DVec3::NEG_Z,
        ] {
            let winding = make_base_winding(normal, 0.0);
            let area = polygon_area_for_test(&winding);
            assert!(
                area > 1.0,
                "winding for normal {normal} has area {area}, expected large"
            );
        }
    }

    #[test]
    fn clip_winding_to_half_spaces_applies_planes_in_sequence() {
        let winding = vec![
            DVec3::new(-2.0, -2.0, 0.0),
            DVec3::new(2.0, -2.0, 0.0),
            DVec3::new(2.0, 2.0, 0.0),
            DVec3::new(-2.0, 2.0, 0.0),
        ];
        let planes = [(DVec3::X, 0.0), (DVec3::Y, 0.0)];

        let clipped = clip_winding_to_half_spaces(winding, &planes, 0.01)
            .expect("winding should survive both clips");

        for v in &clipped {
            assert!(v.x >= -0.01, "vertex {v} behind X plane");
            assert!(v.y >= -0.01, "vertex {v} behind Y plane");
        }
    }

    fn polygon_area_for_test(vertices: &[DVec3]) -> f64 {
        if vertices.len() < 3 {
            return 0.0;
        }

        let mut area = DVec3::ZERO;
        let v0 = vertices[0];
        for i in 1..vertices.len() - 1 {
            let edge1 = vertices[i] - v0;
            let edge2 = vertices[i + 1] - v0;
            area += edge1.cross(edge2);
        }
        area.length() * 0.5
    }
}
