// Exact convex-polytope volume from a plane set, by clipping its bounding box.
// See: context/lib/entity_model.md §7 (Environment Volumes)

use glam::Vec3;

use crate::leaf::LeafPlane;

/// Relative tolerance for "on the plane" during clipping, scaled by the box's
/// half extent so large solids keep the same relative precision.
const RELATIVE_PLANE_EPSILON: f32 = 1.0e-6;

/// Volume of the intersection of `planes`' inner half-spaces with the box
/// `[min, max]`. The box must enclose the solid for the result to be the solid's
/// own volume; a leaf's AABB does by construction. Build-time only: allocates.
///
/// Clips in the box's own frame, centred on the box, so the clipping arithmetic
/// does not grow with distance from the world origin: copies whose stored
/// planes and bounds are exactly congruent compute the same volume wherever
/// they sit. Map-scaled coordinates are rarely exact, so near-identical copies
/// can still differ in the last bits, and overlap ranking sees that difference.
pub(crate) fn clipped_box_volume(planes: &[LeafPlane], min: Vec3, max: Vec3) -> f32 {
    if !min.cmple(max).all() {
        return 0.0;
    }
    let centre = (min + max) * 0.5;
    let (min, max) = (min - centre, max - centre);
    let scale = min.abs().max(max.abs()).max_element().max(1.0);
    let epsilon = RELATIVE_PLANE_EPSILON * scale;

    let mut faces = box_faces(min, max);
    for plane in planes {
        let length = plane.normal.length();
        // Skip a degenerate, NaN or non-finite plane.
        if length.is_nan() || length <= 0.0 || !plane.distance.is_finite() {
            continue;
        }
        let normal = plane.normal / length;
        let distance = (plane.distance - plane.normal.dot(centre)) / length;
        faces = clip_polyhedron(faces, normal, distance, epsilon);
        if faces.is_empty() {
            return 0.0;
        }
    }
    polyhedron_volume(&faces)
}

fn box_faces(min: Vec3, max: Vec3) -> Vec<Vec<Vec3>> {
    let corner = |x: bool, y: bool, z: bool| {
        Vec3::new(
            if x { max.x } else { min.x },
            if y { max.y } else { min.y },
            if z { max.z } else { min.z },
        )
    };
    vec![
        vec![
            corner(false, false, false),
            corner(false, true, false),
            corner(false, true, true),
            corner(false, false, true),
        ],
        vec![
            corner(true, false, false),
            corner(true, false, true),
            corner(true, true, true),
            corner(true, true, false),
        ],
        vec![
            corner(false, false, false),
            corner(false, false, true),
            corner(true, false, true),
            corner(true, false, false),
        ],
        vec![
            corner(false, true, false),
            corner(true, true, false),
            corner(true, true, true),
            corner(false, true, true),
        ],
        vec![
            corner(false, false, false),
            corner(true, false, false),
            corner(true, true, false),
            corner(false, true, false),
        ],
        vec![
            corner(false, false, true),
            corner(false, true, true),
            corner(true, true, true),
            corner(true, false, true),
        ],
    ]
}

/// Clip every face against the half-space `dot(p, normal) <= distance` and close
/// the cut with a cap polygon on the plane. Takes the faces by value: a plane
/// that cuts nothing (most of a brush's own planes against its own AABB) hands
/// them straight back, with no copy.
fn clip_polyhedron(
    faces: Vec<Vec<Vec3>>,
    normal: Vec3,
    distance: f32,
    epsilon: f32,
) -> Vec<Vec<Vec3>> {
    let all_inside = faces
        .iter()
        .flatten()
        .all(|point| normal.dot(*point) - distance <= epsilon);
    if all_inside {
        return faces;
    }

    let mut clipped = Vec::with_capacity(faces.len() + 1);
    let mut cap = Vec::new();
    for face in &faces {
        let polygon = clip_polygon(face, normal, distance, epsilon, &mut cap);
        if polygon.len() >= 3 {
            clipped.push(polygon);
        }
    }
    if let Some(cap) = order_cap(cap, normal, epsilon) {
        clipped.push(cap);
    }
    clipped
}

/// Sutherland–Hodgman against one plane. Points on the plane, kept or created,
/// are also collected into `cap`.
fn clip_polygon(
    polygon: &[Vec3],
    normal: Vec3,
    distance: f32,
    epsilon: f32,
    cap: &mut Vec<Vec3>,
) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(polygon.len() + 1);
    for (index, &current) in polygon.iter().enumerate() {
        let next = polygon[(index + 1) % polygon.len()];
        let current_side = normal.dot(current) - distance;
        let next_side = normal.dot(next) - distance;
        let current_inside = current_side <= epsilon;
        let next_inside = next_side <= epsilon;
        if current_inside {
            out.push(current);
            if current_side.abs() <= epsilon {
                cap.push(current);
            }
        }
        if current_inside != next_inside {
            let t = current_side / (current_side - next_side);
            let crossing = current + (next - current) * t;
            out.push(crossing);
            cap.push(crossing);
        }
    }
    out
}

/// Deduplicate the cut points and order them around their centroid so they form
/// a convex polygon on the plane.
fn order_cap(mut points: Vec<Vec3>, normal: Vec3, epsilon: f32) -> Option<Vec<Vec3>> {
    let mut unique: Vec<Vec3> = Vec::with_capacity(points.len());
    for point in points.drain(..) {
        if !unique
            .iter()
            .any(|kept| kept.distance_squared(point) <= epsilon * epsilon)
        {
            unique.push(point);
        }
    }
    if unique.len() < 3 {
        return None;
    }
    let centroid = unique.iter().copied().sum::<Vec3>() / unique.len() as f32;
    let u = normal.any_orthonormal_vector();
    let v = normal.cross(u);
    unique.sort_by(|a, b| {
        let angle_a = (*a - centroid).dot(v).atan2((*a - centroid).dot(u));
        let angle_b = (*b - centroid).dot(v).atan2((*b - centroid).dot(u));
        angle_a.total_cmp(&angle_b)
    });
    Some(unique)
}

/// Sum of tetrahedra from an interior point to each face's triangle fan. The
/// absolute value per tetrahedron makes the sum independent of face winding.
fn polyhedron_volume(faces: &[Vec<Vec3>]) -> f32 {
    let vertex_count: usize = faces.iter().map(Vec::len).sum();
    if vertex_count == 0 {
        return 0.0;
    }
    let interior = faces.iter().flatten().copied().sum::<Vec3>() / vertex_count as f32;
    let mut volume = 0.0;
    for face in faces {
        let anchor = face[0] - interior;
        for pair in face[1..].windows(2) {
            let b = pair[0] - interior;
            let c = pair[1] - interior;
            volume += anchor.dot(b.cross(c)).abs();
        }
    }
    volume / 6.0
}
