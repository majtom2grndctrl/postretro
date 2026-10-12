// Fluid boundary crossings along a ray: entity unions, adjacent entities, the start-on-face rule.
// See: context/plans/in-progress/E24--water-and-environment-volumes/research.md §Ordering pins (P6, P7)

use glam::Vec3;

use crate::crossings::{FluidCrossing, visit_fluid_crossings};
use crate::leaf::LeafPlane;
use crate::line::CLIP_LINE_CALLS;
use crate::resolve::EnvironmentView;
use crate::test_fixtures::{axis_box, volume};
use crate::volumes::{EnvironmentVolume, EnvironmentVolumes};

const EPS: f32 = 1.0e-5;

fn fluid(brushes: Vec<crate::leaf::ConvexLeaf>, fluid: u16) -> EnvironmentVolume {
    EnvironmentVolume {
        fluid: Some(fluid),
        ..volume(brushes)
    }
}

fn crossings(
    volumes: &EnvironmentVolumes,
    origin: Vec3,
    dir: Vec3,
    max_t: f32,
) -> Vec<FluidCrossing> {
    let mut out = Vec::new();
    visit_fluid_crossings(
        EnvironmentView::new(-9.81, volumes, &[]),
        origin,
        dir,
        max_t,
        |crossing| out.push(crossing),
    );
    out.sort_by(|a, b| a.t.total_cmp(&b.t).then(a.volume.cmp(&b.volume)));
    out
}

fn assert_crossing(
    crossing: &FluidCrossing,
    t: f32,
    normal: Vec3,
    entering: bool,
    fluid: u16,
    volume: usize,
) {
    assert!(
        (crossing.t - t).abs() < EPS,
        "t {} expected {t}",
        crossing.t
    );
    assert!(
        (crossing.outward_normal - normal).length() < EPS,
        "normal {:?} expected {normal:?}",
        crossing.outward_normal
    );
    assert_eq!(crossing.entering, entering);
    assert_eq!(crossing.fluid, fluid);
    assert_eq!(crossing.volume, volume);
}

/// A 10 x 10 pool spanning y 0..2.
fn pool_volumes() -> EnvironmentVolumes {
    EnvironmentVolumes::build(vec![fluid(
        vec![axis_box(
            Vec3::new(-5.0, 0.0, -5.0),
            Vec3::new(5.0, 2.0, 5.0),
        )],
        7,
    )])
}

#[test]
fn ray_from_air_into_fluid_crosses_once_at_the_surface() {
    let volumes = pool_volumes();
    let found = crossings(&volumes, Vec3::new(1.0, 5.0, 0.0), Vec3::NEG_Y, 4.0);
    assert_eq!(found.len(), 1);
    assert_crossing(&found[0], 3.0, Vec3::Y, true, 7, 0);
    assert!((found[0].point - Vec3::new(1.0, 2.0, 0.0)).length() < EPS);
}

#[test]
fn ray_from_inside_fluid_out_through_a_face_crosses_once() {
    let volumes = pool_volumes();
    let found = crossings(&volumes, Vec3::new(0.0, 1.0, 0.0), Vec3::Y, 5.0);
    assert_eq!(found.len(), 1);
    assert_crossing(&found[0], 1.0, Vec3::Y, false, 7, 0);
}

#[test]
fn ray_through_a_floating_block_crosses_twice_with_each_faces_normal() {
    let volumes = EnvironmentVolumes::build(vec![fluid(
        vec![axis_box(
            Vec3::new(-1.0, 2.0, -1.0),
            Vec3::new(1.0, 3.0, 1.0),
        )],
        1,
    )]);
    let found = crossings(&volumes, Vec3::ZERO, Vec3::Y, 10.0);
    assert_eq!(found.len(), 2);
    assert_crossing(&found[0], 2.0, Vec3::NEG_Y, true, 1, 0);
    assert_crossing(&found[1], 3.0, Vec3::Y, false, 1, 0);
}

// Ordering pin P7: the union of one entity's brushes.
#[test]
fn ray_across_two_brushes_of_one_fluid_entity_has_no_crossing_at_their_shared_face() {
    let volumes = EnvironmentVolumes::build(vec![fluid(
        vec![
            axis_box(Vec3::new(-4.0, 0.0, -4.0), Vec3::new(0.0, 2.0, 4.0)),
            axis_box(Vec3::new(0.0, 0.0, -4.0), Vec3::new(4.0, 2.0, 4.0)),
        ],
        0,
    )]);
    let found = crossings(&volumes, Vec3::new(-10.0, 1.0, 0.5), Vec3::X, 20.0);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_crossing(&found[0], 6.0, Vec3::NEG_X, true, 0, 0);
    assert_crossing(&found[1], 14.0, Vec3::X, false, 0, 0);

    // A hairline gap from float noise in the compiled shared plane is no seam either.
    let mut noisy = axis_box(Vec3::new(0.0, 0.0, -4.0), Vec3::new(4.0, 2.0, 4.0));
    noisy.planes[1] = LeafPlane {
        normal: Vec3::NEG_X,
        distance: -2.0e-5,
    };
    let volumes = EnvironmentVolumes::build(vec![fluid(
        vec![
            axis_box(Vec3::new(-4.0, 0.0, -4.0), Vec3::new(0.0, 2.0, 4.0)),
            noisy,
        ],
        0,
    )]);
    assert_eq!(
        crossings(&volumes, Vec3::new(-10.0, 1.0, 0.5), Vec3::X, 20.0).len(),
        2
    );
}

#[test]
fn ray_across_two_adjacent_fluid_entities_crosses_twice_at_their_shared_face() {
    let volumes = EnvironmentVolumes::build(vec![
        fluid(
            vec![axis_box(
                Vec3::new(-4.0, 0.0, -4.0),
                Vec3::new(0.0, 2.0, 4.0),
            )],
            0,
        ),
        fluid(
            vec![axis_box(
                Vec3::new(0.0, 0.0, -4.0),
                Vec3::new(4.0, 2.0, 4.0),
            )],
            3,
        ),
    ]);
    let found = crossings(&volumes, Vec3::new(-10.0, 1.0, 0.5), Vec3::X, 20.0);
    assert_eq!(found.len(), 4, "{found:?}");
    assert_crossing(&found[0], 6.0, Vec3::NEG_X, true, 0, 0);
    assert_crossing(&found[1], 10.0, Vec3::X, false, 0, 0);
    assert_crossing(&found[2], 10.0, Vec3::NEG_X, true, 3, 1);
    assert_crossing(&found[3], 14.0, Vec3::X, false, 3, 1);
}

#[test]
fn ray_starting_on_a_fluid_face_never_crosses_there() {
    let volumes = pool_volumes();
    // On the top face, heading down into the water: only the bottom exit.
    let found = crossings(&volumes, Vec3::new(0.0, 2.0, 0.0), Vec3::NEG_Y, 10.0);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_crossing(&found[0], 2.0, Vec3::NEG_Y, false, 7, 0);
    // On the top face, heading up out of the water: nothing.
    assert!(crossings(&volumes, Vec3::new(0.0, 2.0, 0.0), Vec3::Y, 10.0).is_empty());
    // Within float noise of the face, either side: still nothing there.
    let just_above = crossings(&volumes, Vec3::new(0.0, 2.0005, 0.0), Vec3::NEG_Y, 10.0);
    assert_eq!(just_above.len(), 1, "{just_above:?}");
    assert!(!just_above[0].entering, "only the bottom exit");
    assert!(crossings(&volumes, Vec3::new(0.0, 1.9995, 0.0), Vec3::Y, 10.0).is_empty());
}

#[test]
fn ray_reports_crossings_up_to_and_including_max_t_only() {
    let volumes = pool_volumes();
    let origin = Vec3::new(0.0, 5.0, 0.0);
    assert!(crossings(&volumes, origin, Vec3::NEG_Y, 2.9).is_empty());
    assert_eq!(crossings(&volumes, origin, Vec3::NEG_Y, 3.0).len(), 1);
    // An unnormalized direction measures t in its own units.
    let found = crossings(&volumes, origin, Vec3::new(0.0, -2.0, 0.0), 10.0);
    assert_eq!(found.len(), 2);
    assert_crossing(&found[0], 1.5, Vec3::Y, true, 7, 0);
    assert_crossing(&found[1], 2.5, Vec3::NEG_Y, false, 7, 0);
}

#[test]
fn ray_only_touching_a_fluid_edge_or_missing_it_crosses_nothing() {
    let volumes = pool_volumes();
    // Touching only the top edge at x = 5, y = 2: a zero-length touch.
    assert!(
        crossings(
            &volumes,
            Vec3::new(3.0, 4.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            30.0
        )
        .is_empty()
    );
    // Lying in the top face's plane is inside the closed set, as a point on a
    // face is: the ray enters and leaves through the side faces' edges.
    assert_eq!(
        crossings(&volumes, Vec3::new(-10.0, 2.0, 0.0), Vec3::X, 30.0).len(),
        2
    );
    // Passing above it.
    assert!(crossings(&volumes, Vec3::new(-10.0, 3.0, 0.0), Vec3::X, 30.0).is_empty());
    // A dry volume is never crossed.
    let dry = EnvironmentVolumes::build(vec![EnvironmentVolume {
        gravity: Some(-1.0),
        ..volume(vec![axis_box(Vec3::ZERO, Vec3::ONE)])
    }]);
    assert!(crossings(&dry, Vec3::new(-1.0, 0.5, 0.5), Vec3::X, 5.0).is_empty());
}

#[test]
fn overlapping_fluid_entities_each_report_their_own_boundary_and_fluid() {
    // A small high-priority fluid inside a larger one: each volume is measured
    // on its own, so a ray through both reports four crossings.
    let mut inner = fluid(
        vec![axis_box(
            Vec3::new(-1.0, 0.0, -1.0),
            Vec3::new(1.0, 1.0, 1.0),
        )],
        2,
    );
    inner.priority = 5;
    let volumes = EnvironmentVolumes::build(vec![
        fluid(
            vec![axis_box(
                Vec3::new(-5.0, 0.0, -5.0),
                Vec3::new(5.0, 1.0, 5.0),
            )],
            1,
        ),
        inner,
    ]);
    let found = crossings(&volumes, Vec3::new(-10.0, 0.5, 0.0), Vec3::X, 20.0);
    let summary: Vec<_> = found
        .iter()
        .map(|c| (c.volume, c.fluid, c.entering))
        .collect();
    assert_eq!(
        summary,
        vec![(0, 1, true), (1, 2, true), (1, 2, false), (0, 1, false)]
    );
}

// Regression: the seam gap was measured along the ray, so a ray grazing a
// tilted seam with plane noise saw a gap of noise / sin(angle) and splashed
// inside the pool.
#[test]
fn ray_grazing_a_tilted_seam_between_two_brushes_of_one_fluid_has_no_crossing_there() {
    let normal = Vec3::new(1.0, 1.0, 0.0).normalize();
    let tangent = Vec3::new(1.0, -1.0, 0.0).normalize();
    let split = |gap: f32| {
        let mut below = axis_box(Vec3::splat(-4.0), Vec3::splat(4.0));
        below.planes.push(LeafPlane {
            normal,
            distance: 0.0,
        });
        let mut above = axis_box(Vec3::splat(-4.0), Vec3::splat(4.0));
        above.planes.push(LeafPlane {
            normal: -normal,
            distance: -gap,
        });
        EnvironmentVolumes::build(vec![fluid(vec![below, above], 0)])
    };
    // Crossing the seam plane at 3 degrees, mid-box.
    let angle = 3.0_f32.to_radians();
    let dir = tangent * angle.cos() + normal * angle.sin();
    let origin = Vec3::new(0.0, 0.0, 0.5) - dir * 20.0;

    // 20 microns of plane noise: about 0.4 mm along this ray.
    let noisy = crossings(&split(2.0e-5), origin, dir, 40.0);
    assert_eq!(noisy.len(), 2, "{noisy:?}");
    assert!(noisy[0].entering && !noisy[1].entering);

    // A real 1 cm gap between the brushes still splashes out and back in.
    let gapped = crossings(&split(0.01), origin, dir, 40.0);
    assert_eq!(gapped.len(), 4, "{gapped:?}");

    // A non-finite origin crosses nothing.
    assert!(crossings(&split(0.0), Vec3::new(f32::NAN, 0.0, 0.5), dir, 40.0).is_empty());
}

// A ray meeting more of one volume's brushes than the measurement keeps inline
// reports the same crossings through the re-clip fallback.
#[test]
fn ray_along_a_many_brush_fluid_volume_reports_its_union_boundary_only() {
    // Twenty 1 m blocks along x, each overlapping the next by 0.1 m, with real
    // 0.5 m gaps after the 7th and 14th: three runs.
    let mut starts = Vec::new();
    let mut x = 0.0;
    for block in 0..20 {
        starts.push(x);
        x += if block == 6 || block == 13 { 1.5 } else { 0.9 };
    }
    let brushes = starts
        .iter()
        .map(|&x| axis_box(Vec3::new(x, 0.0, -1.0), Vec3::new(x + 1.0, 1.0, 1.0)))
        .collect();
    let volumes = EnvironmentVolumes::build(vec![fluid(brushes, 2)]);
    let found = crossings(&volumes, Vec3::new(-1.0, 0.5, 0.0), Vec3::X, 40.0);
    let millimetres = |t: f32| (t * 1000.0).round() as i32;
    let summary: Vec<_> = found
        .iter()
        .map(|c| (millimetres(c.t), c.entering))
        .collect();
    // The ray starts 1 m before x = 0.
    let at = |x: f32| millimetres(x + 1.0);
    assert_eq!(
        summary,
        vec![
            (at(starts[0]), true),
            (at(starts[6] + 1.0), false),
            (at(starts[7]), true),
            (at(starts[13] + 1.0), false),
            (at(starts[14]), true),
            (at(starts[19] + 1.0), false),
        ],
        "{found:?}"
    );
}

/// Clips `visit_fluid_crossings` makes on this thread for one ray.
fn crossings_counting_clips(
    volumes: &EnvironmentVolumes,
    origin: Vec3,
    dir: Vec3,
    max_t: f32,
) -> (Vec<FluidCrossing>, usize) {
    let before = CLIP_LINE_CALLS.with(|calls| calls.get());
    let found = crossings(volumes, origin, dir, max_t);
    (found, CLIP_LINE_CALLS.with(|calls| calls.get()) - before)
}

// Regression: measuring a volume clipped every brush below and after the first
// one the ray hit, so a ray's cost grew with the volume's total brush count.
#[test]
fn ray_meeting_few_brushes_of_a_400_brush_fluid_volume_clips_only_those_near_it() {
    // 400 blocks in a column along z, far from both rays, then some moved onto
    // them: ray A meets brushes 397..400 (inline gather), ray B meets 17
    // brushes spread from index 60 up (more than are kept inline, so the
    // re-clip fallback runs).
    let mut brushes: Vec<_> = (0..400)
        .map(|i| {
            let z = 2.0 * i as f32;
            axis_box(Vec3::new(0.0, 0.0, z), Vec3::new(1.0, 1.0, z + 1.0))
        })
        .collect();
    let on_ray =
        |y: f32, x0: f32, x1: f32| axis_box(Vec3::new(x0, y, 0.0), Vec3::new(x1, y + 1.0, 1.0));
    // A: 397 and 398 share a face (one run), 399 stands apart.
    brushes[397] = on_ray(10.0, 0.0, 1.0);
    brushes[398] = on_ray(10.0, 1.0, 2.0);
    brushes[399] = on_ray(10.0, 3.0, 4.0);
    // B: brush 60 + 20k at x = 1.5k, each its own run.
    let spread: Vec<usize> = (0..17).map(|k| 60 + 20 * k).collect();
    for (k, &index) in spread.iter().enumerate() {
        let x = 1.5 * k as f32;
        brushes[index] = on_ray(20.0, x, x + 1.0);
    }
    let volumes = EnvironmentVolumes::build(vec![fluid(brushes, 4)]);

    let (found, clips) =
        crossings_counting_clips(&volumes, Vec3::new(-1.0, 10.5, 0.5), Vec3::X, 10.0);
    assert_eq!(found.len(), 4, "{found:?}");
    assert_crossing(&found[0], 1.0, Vec3::NEG_X, true, 4, 0);
    assert_crossing(&found[1], 3.0, Vec3::X, false, 4, 0);
    assert_crossing(&found[2], 4.0, Vec3::NEG_X, true, 4, 0);
    assert_crossing(&found[3], 5.0, Vec3::X, false, 4, 0);
    // Brush 397 is clipped by its visit and the gather's three clips; 398 and
    // 399 by their visits and a gate that stops at 397. None of the 397 the
    // ray misses is clipped.
    assert!(clips <= 8, "ray A made {clips} clips");

    let (found, clips) =
        crossings_counting_clips(&volumes, Vec3::new(-1.0, 20.5, 0.5), Vec3::X, 40.0);
    assert_eq!(found.len(), 2 * spread.len(), "{found:?}");
    for (k, pair) in found.chunks(2).enumerate() {
        let x = 1.5 * k as f32;
        assert!(
            (pair[0].t - (x + 1.0)).abs() < EPS && pair[0].entering,
            "{pair:?}"
        );
        assert!(
            (pair[1].t - (x + 2.0)).abs() < EPS && !pair[1].entering,
            "{pair:?}"
        );
    }
    // The fallback re-clips on every run-extension pass (a few per run), but
    // only the 17 brushes on the ray: without the bounds test each pass would
    // clip every brush from index 60 on (~340).
    let n = spread.len();
    assert!(clips <= 4 * (n + 1) * n, "ray B made {clips} clips");
}
