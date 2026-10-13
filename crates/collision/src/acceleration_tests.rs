use super::*;
use crate::bvh::AabbTree;
use crate::leaf::LeafPlane;
use crate::test_fixtures::{axis_box, volume};
use crate::{EnvironmentView, EnvironmentVolumes, resolve_particle_acceleration};

#[test]
fn unit_axis_bounds_match_plane_arithmetic_at_float_boundaries() {
    for axis in 0..3 {
        for sign in [-1.0, 1.0] {
            let mut normal = Vec3::splat(-0.0);
            normal[axis] = sign;
            for distance in [
                0.0,
                -0.0,
                f32::from_bits(1),
                -f32::from_bits(1),
                1.0,
                -1.0,
                1.0e30,
                -1.0e30,
                f32::MAX,
                -f32::MAX,
            ] {
                let leaf = ConvexLeaf {
                    planes: vec![LeafPlane { normal, distance }],
                    aabb_min: Vec3::splat(-f32::MAX),
                    aabb_max: Vec3::splat(f32::MAX),
                };
                let (min, max) = axis_bounds(&leaf).expect("unit axis");
                let face = distance * sign;
                for coordinate in [face.next_down(), face, face.next_up()] {
                    if !coordinate.is_finite() {
                        continue;
                    }
                    for other in [0.0, -0.0, f32::from_bits(1), -f32::MAX, f32::MAX] {
                        let mut point = Vec3::splat(other);
                        point[axis] = coordinate;
                        assert_eq!(
                            point.cmpge(min).all() && point.cmple(max).all(),
                            leaf.contains_point(point),
                            "axis {axis}, sign {sign}, distance {distance}, point {point:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn arbitrary_planes_retain_exact_narrow_phase_and_independent_fields() {
    let mut wedge = axis_box(Vec3::ZERO, Vec3::ONE);
    wedge.planes.push(LeafPlane {
        normal: Vec3::new(1.0, 1.0, 0.0),
        distance: 1.0,
    });
    assert!(axis_bounds(&wedge).is_none());
    let volumes = EnvironmentVolumes::build(vec![
        EnvironmentVolume {
            gravity: Some(-2.0),
            priority: 1,
            ..volume(vec![wedge])
        },
        EnvironmentVolume {
            push: Some(Vec3::X * 3.0),
            ..volume(vec![axis_box(Vec3::ZERO, Vec3::ONE)])
        },
    ]);
    let view = EnvironmentView::new(-9.0, &volumes, &[]);
    let inside = resolve_particle_acceleration(view, Vec3::new(0.25, 0.25, 0.5));
    let outside = resolve_particle_acceleration(view, Vec3::new(0.75, 0.75, 0.5));
    assert!((inside.0 + 2.0).abs() < 1.0e-6);
    assert!((outside.0 + 9.0).abs() < 1.0e-6);
    assert!((inside.1 - Vec3::X * 3.0).length() < 1.0e-6);
    assert!((outside.1 - inside.1).length() < 1.0e-6);

    for plane in [
        LeafPlane {
            normal: Vec3::X * 2.0,
            distance: 1.0,
        },
        LeafPlane {
            normal: Vec3::X,
            distance: f32::NAN,
        },
        LeafPlane {
            normal: Vec3::splat(f32::NAN),
            distance: 1.0,
        },
    ] {
        let mut leaf = axis_box(Vec3::ZERO, Vec3::ONE);
        leaf.planes.push(plane);
        assert!(!acceleration_bounds(&leaf).2);
    }
}

#[test]
fn malformed_supplied_bounds_preserve_original_tree_prefilter() {
    for (min, max) in [
        (Vec3::splat(f32::NAN), Vec3::ONE),
        (Vec3::ZERO, Vec3::splat(f32::NAN)),
        (Vec3::splat(f32::NEG_INFINITY), Vec3::splat(f32::INFINITY)),
        (Vec3::ONE, Vec3::ZERO),
    ] {
        let mut leaf = axis_box(Vec3::ZERO, Vec3::ONE);
        leaf.aabb_min = min;
        leaf.aabb_max = max;
        assert!(!acceleration_bounds(&leaf).2);
        assert_original_prefilter(&leaf);
    }
    let mut contradictory = axis_box(Vec3::ZERO, Vec3::ONE);
    contradictory.planes.push(LeafPlane {
        normal: Vec3::X,
        distance: -1.0,
    });
    assert!(!acceleration_bounds(&contradictory).2);
    assert_original_prefilter(&contradictory);
}

fn assert_original_prefilter(leaf: &ConvexLeaf) {
    let (min, max) = leaf.padded_bounds();
    let tree = AabbTree::build(vec![TreeEntry {
        min,
        max,
        payload: 0,
    }]);
    let volumes = vec![EnvironmentVolume {
        gravity: Some(-2.0),
        ..volume(vec![leaf.clone()])
    }];
    let index = AccelerationIndex::build(&volumes, &[0]);
    for point in [
        Vec3::ZERO,
        Vec3::splat(0.5),
        Vec3::ONE,
        Vec3::splat(-1.0),
        Vec3::splat(2.0),
        Vec3::splat(f32::NAN),
        Vec3::splat(f32::INFINITY),
    ] {
        let mut expected = Vec::new();
        tree.visit_point(point, |payload| {
            if leaf.contains_point(point) {
                expected.push(*payload);
            }
        });
        let mut actual = Vec::new();
        index.visit_containing_point(&volumes, point, ContentsMask::GRAVITY, |payload| {
            actual.push(payload)
        });
        assert_eq!(actual, expected, "point {point:?}");
    }
}
