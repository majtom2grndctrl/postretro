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

// A body feels push × force; a particle follows the nominal push, so a
// decorative (force 0) current still carries foam.
#[test]
fn bodies_feel_push_times_force_while_particles_follow_the_nominal_push() {
    use crate::{BodyProbe, resolve_environment};
    let probe = |position: Vec3| BodyProbe {
        position,
        half_height: 0.5,
        radius: 0.25,
        eye_height: 0.6,
    };
    for force in [0.0, 0.5, 1.0, 3.0] {
        let current = EnvironmentVolume {
            push: Some(Vec3::new(6.0, 0.0, 0.0)),
            force,
            ..volume(vec![axis_box(Vec3::splat(-10.0), Vec3::splat(10.0))])
        };
        let volumes = EnvironmentVolumes::build(vec![current]);
        let view = EnvironmentView::new(-9.81, &volumes, &[]);
        let felt = resolve_environment(view, &probe(Vec3::ZERO)).push;
        assert_eq!(felt, Vec3::new(6.0 * force, 0.0, 0.0), "force {force}");
        let (_, nominal) = resolve_particle_acceleration(view, Vec3::ZERO);
        assert_eq!(nominal, Vec3::new(6.0, 0.0, 0.0), "force {force}");
    }
}

// Production particles call the batch form: every point gets the nominal push,
// whatever the volume's force.
#[test]
fn batched_particle_accelerations_return_the_nominal_push_for_every_point() {
    use crate::resolve_particle_accelerations;
    let current = EnvironmentVolume {
        push: Some(Vec3::new(6.0, 0.0, 2.0)),
        force: 0.5,
        ..volume(vec![axis_box(Vec3::splat(-10.0), Vec3::splat(10.0))])
    };
    let volumes = EnvironmentVolumes::build(vec![current]);
    let view = EnvironmentView::new(-9.81, &volumes, &[]);
    let positions = [Vec3::ZERO, Vec3::new(3.0, -2.0, 5.0), Vec3::splat(9.5)];
    let mut accelerations = [(0.0, Vec3::ZERO); 3];
    resolve_particle_accelerations(view, &positions, &mut accelerations);
    for (&position, &(gravity, push)) in positions.iter().zip(&accelerations) {
        assert_eq!(gravity, -9.81, "point {position:?}");
        assert_eq!(push, Vec3::new(6.0, 0.0, 2.0), "point {position:?}");
    }
}

// The overlap winner is picked on the nominal fields; its own force scales it.
// A force-0 winner inside a stronger push leaves the body feeling nothing.
#[test]
fn a_force_zero_winner_shadows_a_weaker_ranked_push_for_bodies() {
    use crate::{BodyProbe, resolve_environment};
    let wind = EnvironmentVolume {
        push: Some(Vec3::new(8.0, 0.0, 0.0)),
        ..volume(vec![axis_box(Vec3::splat(-10.0), Vec3::splat(10.0))])
    };
    let decorative = EnvironmentVolume {
        push: Some(Vec3::new(0.0, 0.0, 4.0)),
        force: 0.0,
        ..volume(vec![axis_box(Vec3::splat(-1.0), Vec3::splat(1.0))])
    };
    let volumes = EnvironmentVolumes::build(vec![wind, decorative]);
    let view = EnvironmentView::new(-9.81, &volumes, &[]);
    let body = resolve_environment(
        view,
        &BodyProbe {
            position: Vec3::ZERO,
            half_height: 0.5,
            radius: 0.25,
            eye_height: 0.6,
        },
    );
    assert_eq!(body.push, Vec3::ZERO);
    assert_eq!(
        resolve_particle_acceleration(view, Vec3::ZERO).1,
        Vec3::new(0.0, 0.0, 4.0)
    );
}
