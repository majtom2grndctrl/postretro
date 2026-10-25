// A mover ghost's placed bounds (centre, oriented containment, segments) and
// its triangles' answer to what lies between the eye and a rider.
// See: context/lib/rendering_pipeline.md §7.9

use super::*;

/// The twelve triangles of a box's six faces, for building mover shapes.
pub(crate) fn box_triangles(local: Aabb) -> [[Vec3; 3]; 12] {
    let corner = |bits: usize| {
        let high = glam::BVec3::new(bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
        Vec3::select(high, local.max, local.min)
    };
    let faces = [
        [0, 2, 6, 4],
        [1, 5, 7, 3],
        [0, 4, 5, 1],
        [2, 3, 7, 6],
        [0, 1, 3, 2],
        [4, 6, 7, 5],
    ];
    let mut triangles = [[Vec3::ZERO; 3]; 12];
    for (face, [a, b, c, d]) in faces.into_iter().enumerate() {
        triangles[face * 2] = [corner(a), corner(b), corner(c)];
        triangles[face * 2 + 1] = [corner(a), corner(c), corner(d)];
    }
    triangles
}

fn slab() -> Aabb {
    Aabb {
        min: Vec3::new(-1.0, -1.0, -0.05),
        max: Vec3::new(1.0, 1.0, 0.05),
    }
}

// A 2 m slab turned 45° about world up: a point half a metre behind its face
// lies inside the slab's world-axis enclosure but outside the slab, so the
// test runs in the slab's own frame.
#[test]
fn mover_ghost_bounds_test_containment_in_the_turned_box_not_its_world_box() {
    let transform = Mat4::from_translation(Vec3::new(0.0, 0.0, -4.0))
        * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4);
    let bounds = MoverGhostBounds::new(transform, slab());
    assert!(bounds.center().abs_diff_eq(Vec3::new(0.0, 0.0, -4.0), 1e-6));

    let behind = transform.transform_point3(Vec3::new(0.0, 0.0, -0.5));
    assert!(slab().transformed(&transform).min.cmple(behind).all());
    assert!(slab().transformed(&transform).max.cmpge(behind).all());
    assert!(!bounds.contains(behind, 0.1));

    let on_face = transform.transform_point3(Vec3::new(0.5, 0.0, 0.12));
    assert!(bounds.contains(on_face, 0.1));
    assert!(!bounds.contains(on_face, 0.05));
}

// The margin is metres in the world, whatever the transform's scale.
#[test]
fn mover_ghost_bounds_margin_is_world_metres_under_a_scaled_transform() {
    let bounds = MoverGhostBounds::new(Mat4::from_scale(Vec3::splat(4.0)), slab());
    // The face sits at world z 0.2; 0.08 m past it is inside a 0.1 m margin.
    assert!(bounds.contains(Vec3::new(0.0, 0.0, 0.28), 0.1));
    assert!(!bounds.contains(Vec3::new(0.0, 0.0, 0.32), 0.1));
}

// A transform with no inverse, a box that is not finite or is inverted, and a
// non-finite point all hold nothing, without panicking.
#[test]
fn mover_ghost_bounds_with_no_inverse_or_bad_bounds_contain_nothing() {
    let flat = MoverGhostBounds::new(Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0)), slab());
    assert!(!flat.contains(Vec3::ZERO, 0.1));

    let nan = Aabb {
        min: Vec3::splat(f32::NAN),
        max: Vec3::ONE,
    };
    for local in [nan, Aabb::empty()] {
        let bounds = MoverGhostBounds::new(Mat4::IDENTITY, local);
        assert!(!bounds.contains(Vec3::ZERO, 0.1));
        assert!(!bounds.center().is_finite());
    }
    let inverted = Aabb {
        min: Vec3::ONE,
        max: -Vec3::ONE,
    };
    assert!(!MoverGhostBounds::new(Mat4::IDENTITY, inverted).contains(Vec3::ZERO, 0.1));

    let nan_transform =
        MoverGhostBounds::new(Mat4::from_translation(Vec3::splat(f32::NAN)), slab());
    assert!(!nan_transform.contains(Vec3::ZERO, 0.1));

    assert!(!MoverGhostBounds::new(Mat4::IDENTITY, slab()).contains(Vec3::NAN, 0.1));
}

// A segment meets the turned slab where it crosses it, at an end inside it,
// or within the margin of a face; it misses when it stops short, passes
// beside, or runs parallel outside a face.
#[test]
fn mover_ghost_bounds_meet_a_segment_crossing_the_turned_box() {
    let transform = Mat4::from_translation(Vec3::new(0.0, 0.0, -4.0))
        * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4);
    let bounds = MoverGhostBounds::new(transform, slab());
    let local = |x: f32, y: f32, z: f32| transform.transform_point3(Vec3::new(x, y, z));

    assert!(bounds.meets_segment(local(0.0, 0.0, 2.0), local(0.0, 0.0, -2.0), 0.0));
    assert!(bounds.meets_segment(local(0.2, 0.3, 0.0), local(0.0, 0.0, 5.0), 0.0));
    assert!(!bounds.meets_segment(local(0.0, 0.0, 2.0), local(0.0, 0.0, 0.5), 0.0));
    assert!(!bounds.meets_segment(local(1.5, 0.0, 2.0), local(1.5, 0.0, -2.0), 0.0));
    // Parallel to the face, 0.08 m in front of it.
    let along_face = (local(-2.0, 0.0, 0.13), local(2.0, 0.0, 0.13));
    assert!(!bounds.meets_segment(along_face.0, along_face.1, 0.0));
    assert!(bounds.meets_segment(along_face.0, along_face.1, 0.1));
    // A point segment asks for containment.
    assert!(bounds.meets_segment(local(0.5, 0.0, 0.0), local(0.5, 0.0, 0.0), 0.0));
}

// No segment meets a box with no inverse or bounds, and a segment with a
// non-finite end meets nothing, without panicking.
#[test]
fn mover_ghost_bounds_meet_no_segment_without_a_sound_box_or_finite_ends() {
    let through = (Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.0, 0.0, -2.0));
    let flat = MoverGhostBounds::new(Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0)), slab());
    assert!(!flat.meets_segment(through.0, through.1, 0.1));
    let inverted = Aabb {
        min: Vec3::ONE,
        max: -Vec3::ONE,
    };
    let inverted = MoverGhostBounds::new(Mat4::IDENTITY, inverted);
    assert!(!inverted.meets_segment(through.0, through.1, 0.1));
    let sound = MoverGhostBounds::new(Mat4::IDENTITY, slab());
    assert!(sound.meets_segment(through.0, through.1, 0.0));
    assert!(!sound.meets_segment(Vec3::NAN, through.1, 0.1));
    assert!(!sound.meets_segment(through.0, Vec3::splat(f32::INFINITY), 0.1));
}

/// A 3 × 3 m lift deck, its top at local y 0, ringed by 1.1 m railings
/// 5 cm thick: its geometry as boxes, and the boxes' enclosure.
fn railed_lift() -> (Vec<[Vec3; 3]>, Aabb) {
    let rail = |min: [f32; 3], max: [f32; 3]| Aabb {
        min: Vec3::from_array(min),
        max: Vec3::from_array(max),
    };
    let parts = [
        rail([-1.5, -0.2, -1.5], [1.5, 0.0, 1.5]),
        rail([-1.5, 0.0, -1.5], [1.5, 1.1, -1.45]),
        rail([-1.5, 0.0, 1.45], [1.5, 1.1, 1.5]),
        rail([-1.5, 0.0, -1.5], [-1.45, 1.1, 1.5]),
        rail([1.45, 0.0, -1.5], [1.5, 1.1, 1.5]),
    ];
    let triangles = parts.into_iter().flat_map(box_triangles).collect();
    (triangles, rail([-1.5, -0.2, -1.5], [1.5, 1.1, 1.5]))
}

// The eye's line to a rider on a railed lift passes inside the lift's box
// but over its railings and above its deck: no surface lies between. A line
// through a railing, through the deck from below, or ending at a rider
// behind a railing does cross one.
#[test]
fn mover_ghost_shape_finds_a_surface_only_where_the_line_crosses_a_triangle() {
    let transform = Mat4::from_translation(Vec3::new(0.0, 0.0, -6.0))
        * Mat4::from_rotation_y(30f32.to_radians());
    let (triangles, local) = railed_lift();
    let lift = MoverGhostShape::new(MoverGhostBounds::new(transform, local), &triangles);
    let at = |x: f32, y: f32, z: f32| transform.transform_point3(Vec3::new(x, y, z));
    let body = at(0.0, 0.92, -1.0);

    let above = at(0.0, 3.0, 6.0);
    assert!(lift.bounds().meets_segment(above, body, 0.0));
    assert!(!lift.surface_between(above, body));

    // Level with the rider's body, from outside the near railing.
    assert!(lift.surface_between(at(0.0, 0.92, 6.0), body));
    // From under the deck.
    assert!(lift.surface_between(at(0.0, -3.0, -1.0), body));
    // Beside the lift: the line never meets its box.
    assert!(!lift.surface_between(at(5.0, 0.92, -1.0), at(4.0, 0.92, -1.0)));
}

// A floor (a quad) crossing the line within a centimetre of the body end, in
// world metres under a scaled transform, does not count; one a collision
// skin (2 cm) from it does, so a floor under a foot-level body point still
// lies between an eye below it and the rider.
#[test]
fn mover_ghost_shape_ignores_a_surface_within_a_centimetre_of_the_body_end() {
    let floor = Aabb {
        min: Vec3::new(-0.5, 0.0, -0.5),
        max: Vec3::new(0.5, 0.0, 0.5),
    };
    let triangles = box_triangles(floor);
    let scaled = Mat4::from_scale(Vec3::splat(4.0));
    let shape = MoverGhostShape::new(MoverGhostBounds::new(scaled, floor), &triangles);
    let below = Vec3::new(0.3, -3.0, 0.2);

    assert!(!shape.surface_between(below, Vec3::new(0.0, 0.005, 0.0)));
    assert!(shape.surface_between(below, Vec3::new(0.0, 0.02, 0.0)));
}

// A line parallel to a triangle, a triangle with no area, a non-finite end, a
// box with no inverse, and a line no longer than the end tolerance count no
// surface between, without panicking.
#[test]
fn mover_ghost_shape_counts_no_surface_for_parallel_degenerate_or_non_finite_input() {
    let floor = Aabb {
        min: Vec3::new(-1.0, 0.0, -1.0),
        max: Vec3::new(1.0, 0.0, 1.0),
    };
    let triangles = box_triangles(floor);
    let flat = MoverGhostShape::new(MoverGhostBounds::new(Mat4::IDENTITY, floor), &triangles);
    // Along the floor's plane, through its middle.
    assert!(!flat.surface_between(Vec3::new(-2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0)));
    // Down through it, the control.
    assert!(flat.surface_between(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.1, -1.0, 0.0)));

    let point = [[Vec3::ZERO; 3]];
    let line = [[Vec3::ZERO, Vec3::X, Vec3::X * 2.0]];
    for degenerate in [&point[..], &line[..]] {
        let shape = MoverGhostShape::new(MoverGhostBounds::new(Mat4::IDENTITY, floor), degenerate);
        assert!(!shape.surface_between(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, -1.0, 0.0)));
    }

    assert!(!flat.surface_between(Vec3::NAN, Vec3::new(0.0, -1.0, 0.0)));
    assert!(!flat.surface_between(Vec3::new(0.0, 1.0, 0.0), Vec3::splat(f32::INFINITY)));
    let squashed = MoverGhostShape::new(
        MoverGhostBounds::new(Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0)), floor),
        &triangles,
    );
    assert!(!squashed.surface_between(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, -1.0, 0.0)));
    assert!(!flat.surface_between(Vec3::new(0.0, 0.004, 0.0), Vec3::new(0.0, -0.004, 0.0)));
}
