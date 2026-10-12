// Tree-backed queries against a brute-force scan of every leaf, plus the zero-volume set.
// See: context/plans/in-progress/E24--water-and-environment-volumes/research.md §Ordering pins (P9)

use glam::Vec3;
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

use crate::crossings::{
    CROSSING_SEAM_EPSILON, CROSSING_START_EPSILON, FluidCrossing, visit_fluid_crossings,
};
use crate::eye::{
    EYE_DEPTH_REACH, fluid_at_eye, fluid_volume_plane_depth, fluid_volume_signed_depth,
};
use crate::leaf::{ContentsMask, ConvexLeaf, LeafPlane};
use crate::line::{LineInterval, clip_line};
use crate::resolve::{
    BodyProbe, EnvironmentView, ResolvedEnvironment, resolve_environment, resolve_gravity,
};
use crate::test_fixtures::axis_box;
use crate::volumes::{EnvironmentVolume, EnvironmentVolumes};

const LEVEL_GRAVITY: f32 = -9.81;

// ---- Brute-force reference: every volume, every brush, map order, owned buffers. ----

fn reference_containing(
    volumes: &EnvironmentVolumes,
    point: Vec3,
    mask: ContentsMask,
) -> Vec<usize> {
    (0..volumes.len())
        .filter(|&index| {
            let volume = volumes.volume(index).unwrap();
            volume.contents().intersects(mask)
                && volume
                    .brushes
                    .iter()
                    .any(|brush| brush.contains_point(point))
        })
        .collect()
}

fn reference_winner(
    volumes: &EnvironmentVolumes,
    candidates: impl Iterator<Item = usize>,
) -> Option<usize> {
    candidates.reduce(|best, next| {
        if volumes.outranks(next, best) {
            next
        } else {
            best
        }
    })
}

/// Merged runs of positive-length brush intervals along a line, sorted then
/// swept; a gap joins when `joins(gap_start, gap_end)` accepts it:
/// `(start, start_face, end, end_face)`.
type ReferenceRun = (f32, Option<(u32, u32)>, f32, Option<(u32, u32)>);

fn reference_union_runs(
    mut intervals: Vec<(u32, LineInterval)>,
    joins: impl Fn(f32, f32) -> bool,
) -> Vec<ReferenceRun> {
    intervals.retain(|(_, interval)| interval.has_length());
    intervals.sort_by(|a, b| a.1.t_enter.total_cmp(&b.1.t_enter).then(a.0.cmp(&b.0)));
    let mut runs: Vec<ReferenceRun> = Vec::new();
    for &(brush, interval) in &intervals {
        match runs.last_mut() {
            Some(run) if interval.t_enter <= run.2 || joins(run.2, interval.t_enter) => {
                run.2 = run.2.max(interval.t_exit)
            }
            _ => runs.push((
                interval.t_enter,
                interval.enter_plane.map(|plane| (brush, plane)),
                interval.t_exit,
                None,
            )),
        }
    }
    // End face: the lowest brush index among intervals ending at the run's end.
    for run in &mut runs {
        let (brush, interval) = intervals
            .iter()
            .filter(|(_, interval)| interval.t_exit == run.2)
            .min_by_key(|(brush, _)| *brush)
            .expect("a run ends where one of its intervals does");
        run.3 = interval.exit_plane.map(|plane| (*brush, plane));
    }
    runs
}

/// Clipped intervals of `brushes` along a line, tagged with their index.
fn reference_intervals<'a>(
    brushes: impl Iterator<Item = &'a ConvexLeaf>,
    origin: Vec3,
    dir: Vec3,
    t_min: f32,
    t_max: f32,
) -> Vec<(u32, LineInterval)> {
    brushes
        .enumerate()
        .filter_map(|(brush, leaf)| {
            clip_line(&leaf.planes, origin, dir, t_min, t_max).map(|i| (brush as u32, i))
        })
        .collect()
}

/// Every brush of every volume carrying `fluid`, in map order.
fn fluid_brushes(volumes: &EnvironmentVolumes, fluid: u16) -> impl Iterator<Item = &ConvexLeaf> {
    (0..volumes.len())
        .map(|index| volumes.volume(index).unwrap())
        .filter(move |volume| volume.fluid == Some(fluid))
        .flat_map(|volume| volume.brushes.iter())
}

/// Brute-force vertical eye depth through the union of `fluid`'s brushes.
fn reference_vertical_depth(volumes: &EnvironmentVolumes, fluid: u16, point: Vec3) -> Option<f32> {
    let bottom = point - Vec3::new(0.0, EYE_DEPTH_REACH, 0.0);
    let at = point.y - bottom.y;
    let runs = reference_union_runs(
        reference_intervals(
            fluid_brushes(volumes, fluid),
            bottom,
            Vec3::Y,
            0.0,
            2.0 * EYE_DEPTH_REACH,
        ),
        |a, b| {
            let midpoint = bottom + Vec3::new(0.0, 0.5 * (a + b), 0.0);
            fluid_brushes(volumes, fluid).any(|leaf| leaf.is_near(midpoint, CROSSING_SEAM_EPSILON))
        },
    );
    if let Some(run) = runs.iter().find(|run| run.0 <= at && at <= run.2) {
        return Some(run.2 - at);
    }
    runs.iter()
        .map(|run| if run.2 < at { at - run.2 } else { run.0 - at })
        .reduce(f32::min)
        .map(|gap| -gap)
}

/// Brute-force plane depth over `fluid`'s brushes within the eye reach.
fn reference_plane_depth(volumes: &EnvironmentVolumes, fluid: u16, point: Vec3) -> Option<f32> {
    fluid_brushes(volumes, fluid)
        .filter(|leaf| leaf.bounds_within(point, EYE_DEPTH_REACH))
        .map(|leaf| leaf.plane_depth(point))
        .reduce(f32::max)
}

fn reference_resolve(volumes: &EnvironmentVolumes, probe: &BodyProbe) -> ResolvedEnvironment {
    let at_point = |mask: ContentsMask, has: fn(&EnvironmentVolume) -> bool| {
        reference_winner(
            volumes,
            reference_containing(volumes, probe.position, mask)
                .into_iter()
                .filter(|&i| has(volumes.volume(i).unwrap())),
        )
    };
    let gravity = at_point(ContentsMask::GRAVITY, |v| v.gravity.is_some())
        .and_then(|i| volumes.volume(i).unwrap().gravity)
        .unwrap_or(LEVEL_GRAVITY);
    let push = at_point(ContentsMask::PUSH, |v| v.push.is_some())
        .and_then(|i| volumes.volume(i).unwrap().push)
        .unwrap_or(Vec3::ZERO);

    let reach = probe.half_height + probe.radius;
    let feet = Vec3::new(probe.position.x, probe.position.y - reach, probe.position.z);
    let length = 2.0 * reach;
    let eye = probe.position + Vec3::new(0.0, probe.eye_height, 0.0);
    let never = |_: f32, _: f32| false;
    let (winner, immersion) = if length > 0.0 {
        let winner = reference_winner(
            volumes,
            (0..volumes.len()).filter(|&i| {
                let volume = volumes.volume(i).unwrap();
                volume.fluid.is_some()
                    && !reference_union_runs(
                        reference_intervals(volume.brushes.iter(), feet, Vec3::Y, 0.0, length),
                        never,
                    )
                    .is_empty()
            }),
        );
        // Q3: the union of every volume carrying the winner's fluid.
        let covered: f32 = winner
            .and_then(|i| volumes.volume(i).unwrap().fluid)
            .map(|fluid| {
                reference_union_runs(
                    reference_intervals(fluid_brushes(volumes, fluid), feet, Vec3::Y, 0.0, length),
                    never,
                )
                .iter()
                .map(|run| run.2 - run.0)
                .sum()
            })
            .unwrap_or(0.0);
        (winner, (covered / length).clamp(0.0, 1.0))
    } else {
        let winner = at_point(ContentsMask::FLUID, |v| v.fluid.is_some());
        (winner, if winner.is_some() { 1.0 } else { 0.0 })
    };
    let fluid = winner.and_then(|i| volumes.volume(i).unwrap().fluid);
    ResolvedEnvironment {
        gravity,
        push,
        fluid,
        // The tree side resolves with an empty fluid table.
        viscosity: 0.0,
        immersion,
        tick_eye_in_fluid: fluid.is_some_and(|fluid| {
            fluid_brushes(volumes, fluid).any(|brush| brush.contains_point(eye))
        }),
    }
}

fn reference_crossings(
    volumes: &EnvironmentVolumes,
    origin: Vec3,
    dir: Vec3,
    max_t: f32,
) -> Vec<FluidCrossing> {
    let dir_length = dir.length();
    let start_t = CROSSING_START_EPSILON / dir_length;
    let seam_t = CROSSING_SEAM_EPSILON / dir_length;
    let clip_end = max_t + 4.0 * seam_t;
    let mut out = Vec::new();
    for index in 0..volumes.len() {
        let volume = volumes.volume(index).unwrap();
        let Some(fluid) = volume.fluid else {
            continue;
        };
        let runs = reference_union_runs(
            reference_intervals(volume.brushes.iter(), origin, dir, 0.0, clip_end),
            |a, b| {
                let midpoint = origin + dir * (0.5 * (a + b));
                volume
                    .brushes
                    .iter()
                    .any(|leaf| leaf.is_near(midpoint, CROSSING_SEAM_EPSILON))
            },
        );
        for (start, start_face, end, end_face) in runs {
            for (t, face, entering) in [(start, start_face, true), (end, end_face, false)] {
                let Some((brush, plane)) = face else {
                    continue;
                };
                if t > start_t && t <= max_t {
                    out.push(FluidCrossing {
                        t,
                        point: origin + dir * t,
                        outward_normal: volume.brushes[brush as usize].planes[plane as usize]
                            .normal
                            .normalize_or_zero(),
                        fluid,
                        volume: index,
                        entering,
                    });
                }
            }
        }
    }
    sort_crossings(&mut out);
    out
}

fn sort_crossings(crossings: &mut [FluidCrossing]) {
    crossings.sort_by(|a, b| {
        a.volume
            .cmp(&b.volume)
            .then(a.t.total_cmp(&b.t))
            .then(b.entering.cmp(&a.entering))
    });
}

// ---- Through the tree. ----

fn tree_containing(volumes: &EnvironmentVolumes, point: Vec3, mask: ContentsMask) -> Vec<usize> {
    let mut hits = Vec::new();
    volumes.visit_volumes_containing_point(point, mask, |index| hits.push(index));
    hits.sort_unstable();
    hits.dedup();
    hits
}

fn tree_crossings(
    volumes: &EnvironmentVolumes,
    origin: Vec3,
    dir: Vec3,
    max_t: f32,
) -> Vec<FluidCrossing> {
    let mut out = Vec::new();
    visit_fluid_crossings(
        EnvironmentView::new(LEVEL_GRAVITY, volumes, &[]),
        origin,
        dir,
        max_t,
        |crossing| out.push(crossing),
    );
    sort_crossings(&mut out);
    out
}

// ---- Generators. Coordinates sit on a half-metre grid so queries land exactly
// on faces, edges and shared AABB boundaries; tilted cuts use small integer,
// unnormalized normals so on-face points stay exact. ----

fn grid(range: std::ops::RangeInclusive<i32>) -> impl Strategy<Value = f32> {
    range.prop_map(|step| step as f32 * 0.5)
}

fn grid_point() -> impl Strategy<Value = Vec3> {
    (grid(-18..=18), grid(-18..=18), grid(-18..=18)).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

fn any_point() -> impl Strategy<Value = Vec3> {
    prop_oneof![
        3 => grid_point(),
        1 => (-9.5f32..9.5, -9.5f32..9.5, -9.5f32..9.5).prop_map(|(x, y, z)| Vec3::new(x, y, z)),
    ]
}

fn small_vector() -> impl Strategy<Value = Vec3> {
    (-2i32..=2, -2i32..=2, -2i32..=2)
        .prop_filter("non-zero", |(x, y, z)| (*x, *y, *z) != (0, 0, 0))
        .prop_map(|(x, y, z)| Vec3::new(x as f32, y as f32, z as f32))
}

fn brush() -> impl Strategy<Value = ConvexLeaf> {
    (
        grid_point().prop_map(|p| p.clamp(Vec3::splat(-8.0), Vec3::splat(6.0))),
        (1i32..=8, 1i32..=8, 1i32..=8),
        proptest::option::of((small_vector(), 0i32..=2)),
    )
        .prop_map(|(min, (sx, sy, sz), cut)| {
            let max = min + Vec3::new(sx as f32, sy as f32, sz as f32) * 0.5;
            let mut leaf = axis_box(min, max);
            if let Some((normal, offset)) = cut {
                // Through the box centre, shifted outward: the cut never empties it.
                let centre = (min + max) * 0.5;
                leaf.planes.push(LeafPlane {
                    normal,
                    distance: normal.dot(centre) + offset as f32 * 0.5,
                });
            }
            leaf
        })
}

fn environment_volume() -> impl Strategy<Value = EnvironmentVolume> {
    (
        proptest::collection::vec(brush(), 1..=3),
        -1i32..=1,
        proptest::option::of(-4i32..=4),
        proptest::option::of(small_vector()),
        proptest::option::of(0u16..3),
    )
        .prop_map(
            |(brushes, priority, gravity, push, fluid)| EnvironmentVolume {
                source_entity: 0,
                priority,
                gravity: gravity.map(|g| g as f32),
                push,
                fluid,
                brushes,
            },
        )
}

fn probe() -> impl Strategy<Value = BodyProbe> {
    (
        any_point(),
        prop_oneof![Just(0.0f32), Just(0.25), Just(0.5), Just(0.9)],
        prop_oneof![Just(0.0f32), Just(0.25), Just(0.5)],
        prop_oneof![Just(0.0f32), Just(0.5), Just(0.7)],
    )
        .prop_map(|(position, half_height, radius, eye_height)| BodyProbe {
            position,
            half_height,
            radius,
            eye_height,
        })
}

fn ray() -> impl Strategy<Value = (Vec3, Vec3, f32)> {
    (
        any_point(),
        small_vector(),
        prop_oneof![Just(0.5f32), Just(1.0), Just(4.0), Just(10.0), Just(30.0)],
    )
}

/// The eye depths against brute force, measured from every fluid volume.
fn assert_eye_depths_match(
    volumes: &EnvironmentVolumes,
    view: EnvironmentView<'_>,
    point: Vec3,
) -> Result<(), TestCaseError> {
    if let Some(eye) = fluid_at_eye(view, point) {
        let reference =
            reference_vertical_depth(volumes, eye.fluid, point).map_or(0.0, |depth| depth.max(0.0));
        prop_assert_eq!(eye.signed_depth, reference, "eye depth at {:?}", point);
    }
    for index in 0..volumes.len() {
        let Some(fluid) = volumes.volume(index).unwrap().fluid else {
            continue;
        };
        prop_assert_eq!(
            fluid_volume_signed_depth(view, index, point),
            reference_vertical_depth(volumes, fluid, point),
            "vertical depth of volume {} at {:?}",
            index,
            point
        );
        prop_assert_eq!(
            fluid_volume_plane_depth(view, index, point),
            reference_plane_depth(volumes, fluid, point),
            "plane depth of volume {} at {:?}",
            index,
            point
        );
    }
    Ok(())
}

fn assert_resolves_match(
    tree: &ResolvedEnvironment,
    reference: &ResolvedEnvironment,
    probe: &BodyProbe,
) -> Result<(), TestCaseError> {
    prop_assert_eq!(tree.gravity, reference.gravity, "gravity at {:?}", probe);
    prop_assert_eq!(tree.push, reference.push, "push at {:?}", probe);
    prop_assert_eq!(tree.fluid, reference.fluid, "fluid at {:?}", probe);
    prop_assert!(
        (tree.immersion - reference.immersion).abs() <= 1.0e-6,
        "immersion {} vs {} at {:?}",
        tree.immersion,
        reference.immersion,
        probe
    );
    prop_assert_eq!(
        tree.tick_eye_in_fluid,
        reference.tick_eye_in_fluid,
        "tick eye at {:?}",
        probe
    );
    Ok(())
}

// Q6
#[test]
fn tree_queries_match_brute_force_scan_over_random_overlapping_volumes() {
    let config = Config {
        cases: 384,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::deterministic_rng(RngAlgorithm::ChaCha));
    let strategy = (
        proptest::collection::vec(environment_volume(), 0..=40),
        proptest::collection::vec(any_point(), 16),
        proptest::collection::vec(probe(), 16),
        proptest::collection::vec(ray(), 16),
    );
    runner
        .run(&strategy, |(volume_list, points, probes, rays)| {
            let volumes = EnvironmentVolumes::build(volume_list);
            let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
            let all = ContentsMask::FLUID | ContentsMask::GRAVITY | ContentsMask::PUSH;
            for point in points {
                for mask in [
                    all,
                    ContentsMask::FLUID,
                    ContentsMask::GRAVITY,
                    ContentsMask::PUSH,
                ] {
                    prop_assert_eq!(
                        tree_containing(&volumes, point, mask),
                        reference_containing(&volumes, point, mask),
                        "point {:?} mask {:?}",
                        point,
                        mask
                    );
                }
                let reference_gravity = reference_resolve(
                    &volumes,
                    &BodyProbe {
                        position: point,
                        half_height: 0.0,
                        radius: 0.0,
                        eye_height: 0.0,
                    },
                )
                .gravity;
                prop_assert_eq!(resolve_gravity(view, point), reference_gravity);
                let eye = fluid_at_eye(view, point);
                let reference_eye = reference_winner(
                    &volumes,
                    reference_containing(&volumes, point, ContentsMask::FLUID).into_iter(),
                );
                prop_assert_eq!(eye.map(|e| e.volume), reference_eye, "eye at {:?}", point);
                assert_eye_depths_match(&volumes, view, point)?;
            }
            for probe in probes {
                assert_resolves_match(
                    &resolve_environment(view, &probe),
                    &reference_resolve(&volumes, &probe),
                    &probe,
                )?;
            }
            for (origin, dir, max_t) in rays {
                prop_assert_eq!(
                    tree_crossings(&volumes, origin, dir, max_t),
                    reference_crossings(&volumes, origin, dir, max_t),
                    "ray {:?} dir {:?} max_t {}",
                    origin,
                    dir,
                    max_t
                );
            }
            Ok(())
        })
        .unwrap();
}

// Q6, at a leaf count deep enough for many tree levels.
#[test]
fn tree_queries_match_brute_force_scan_over_a_dense_volume_field() {
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: 4,
            failure_persistence: None,
            ..Config::default()
        },
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );
    let strategy = (
        proptest::collection::vec(environment_volume(), 400),
        proptest::collection::vec(probe(), 200),
        proptest::collection::vec(ray(), 200),
    );
    runner
        .run(&strategy, |(volume_list, probes, rays)| {
            let volumes = EnvironmentVolumes::build(volume_list);
            let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
            for probe in probes {
                assert_resolves_match(
                    &resolve_environment(view, &probe),
                    &reference_resolve(&volumes, &probe),
                    &probe,
                )?;
                // The eye depths of the fluid at the probe, through the deep tree.
                if let Some(eye) = fluid_at_eye(view, probe.position) {
                    prop_assert_eq!(
                        fluid_volume_signed_depth(view, eye.volume, probe.position),
                        reference_vertical_depth(&volumes, eye.fluid, probe.position)
                    );
                    prop_assert_eq!(
                        fluid_volume_plane_depth(view, eye.volume, probe.position),
                        reference_plane_depth(&volumes, eye.fluid, probe.position)
                    );
                }
            }
            for (origin, dir, max_t) in rays {
                prop_assert_eq!(
                    tree_crossings(&volumes, origin, dir, max_t),
                    reference_crossings(&volumes, origin, dir, max_t)
                );
            }
            Ok(())
        })
        .unwrap();
}

// A capsule segment meeting more fluid brushes than the resolve keeps inline
// measures the same union through the tree fallback.
#[test]
fn resolve_matches_brute_force_when_the_segment_meets_more_fluid_brushes_than_it_keeps_inline() {
    // Thirteen overlapping 0.2 m layers up 0..2, every third a second fluid.
    let layers: Vec<EnvironmentVolume> = (0..13)
        .map(|layer| {
            let y = 0.15 * layer as f32;
            EnvironmentVolume {
                source_entity: layer,
                priority: 0,
                gravity: None,
                push: None,
                fluid: Some(u16::from(layer % 3 == 2)),
                brushes: vec![axis_box(
                    Vec3::new(-1.0, y, -1.0),
                    Vec3::new(1.0, y + 0.2, 1.0),
                )],
            }
        })
        .collect();
    let volumes = EnvironmentVolumes::build(layers);
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    for (y, half_height) in [(1.0, 0.6), (0.9, 0.5), (1.4, 0.8)] {
        let probe = BodyProbe {
            position: Vec3::new(0.0, y, 0.0),
            half_height,
            radius: 0.4,
            eye_height: 0.3,
        };
        let resolved = resolve_environment(view, &probe);
        assert_resolves_match(&resolved, &reference_resolve(&volumes, &probe), &probe).unwrap();
        assert_eq!(resolved.fluid, Some(0), "the earliest layer in reach wins");
        assert!(resolved.immersion > 0.0 && resolved.immersion < 1.0);
    }
}

// Q5, research.md P9
#[test]
fn zero_volume_set_builds_an_empty_tree_and_every_query_returns_level_defaults() {
    let volumes = EnvironmentVolumes::build(Vec::new());
    assert_eq!(volumes.leaf_count(), 0);
    assert_eq!(volumes, EnvironmentVolumes::empty());
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    for point in [Vec3::ZERO, Vec3::splat(1.0e4), Vec3::new(-3.0, 2.0, 7.5)] {
        let resolved = resolve_environment(
            view,
            &BodyProbe {
                position: point,
                half_height: 0.9,
                radius: 0.4,
                eye_height: 0.6,
            },
        );
        assert_eq!(
            resolved,
            ResolvedEnvironment {
                gravity: LEVEL_GRAVITY,
                push: Vec3::ZERO,
                fluid: None,
                viscosity: 0.0,
                immersion: 0.0,
                tick_eye_in_fluid: false,
            }
        );
        assert_eq!(resolve_gravity(view, point), LEVEL_GRAVITY);
        assert_eq!(fluid_at_eye(view, point), None);
        assert!(
            tree_containing(&volumes, point, ContentsMask::FLUID | ContentsMask::GRAVITY)
                .is_empty()
        );
        assert!(tree_crossings(&volumes, point, Vec3::new(1.0, -1.0, 0.5), 1.0e4).is_empty());
    }
}
