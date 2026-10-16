use glam::Vec3;

use super::TeleporterRegions;
use crate::leaf::LeafPlane;
use crate::test_fixtures::axis_box;

#[test]
fn teleport_body_point_union_ignores_capsule_overlap_and_gaps() {
    let regions = TeleporterRegions::build(vec![vec![
        axis_box(Vec3::ZERO, Vec3::ONE),
        axis_box(Vec3::new(3.0, 0.0, 0.0), Vec3::new(4.0, 1.0, 1.0)),
    ]]);
    assert_eq!(regions.len(), 1);
    assert!(!regions.is_empty());
    for point in [Vec3::splat(0.5), Vec3::new(3.5, 0.5, 0.5)] {
        assert!(regions.contains(0, point));
        assert_eq!(regions.winner(point, |_| true), Some(0));
    }
    // A body 0.1 m outside could overlap with a 0.3 m capsule, but its body
    // point cannot fire. The gap between disjoint brushes cannot fire either.
    for point in [Vec3::new(-0.1, 0.5, 0.5), Vec3::new(2.0, 0.5, 0.5)] {
        assert!(!regions.contains(0, point));
        assert_eq!(regions.winner(point, |_| true), None);
    }
    assert!(regions.contains(0, Vec3::new(1.0, 0.5, 0.5)));
    assert!(!regions.contains(0, Vec3::new(1.0001, 0.5, 0.5)));
}

#[test]
fn teleport_overlap_winner_uses_convex_volume_and_exact_plane_containment() {
    let cube = axis_box(Vec3::ZERO, Vec3::ONE);
    let mut wedge = cube.clone();
    wedge.planes.push(LeafPlane {
        normal: Vec3::new(1.0, 1.0, 0.0),
        distance: 1.0,
    });
    let regions = TeleporterRegions::build(vec![vec![cube], vec![wedge]]);
    // Bounds match, but the wedge has half the convex volume and wins where
    // both contain the body. Bounds alone cannot admit a point above its slope.
    assert_eq!(regions.winner(Vec3::splat(0.2), |_| true), Some(1));
    assert!(!regions.contains(1, Vec3::splat(0.9)));
    assert_eq!(regions.winner(Vec3::splat(0.9), |_| true), Some(0));
}

#[test]
fn teleport_overlap_winner_sums_overlapping_brushes_instead_of_union_volume() {
    let cube = axis_box(Vec3::ZERO, Vec3::ONE);
    let wider = axis_box(Vec3::ZERO, Vec3::new(1.5, 1.0, 1.0));
    let regions = TeleporterRegions::build(vec![vec![cube.clone(), cube], vec![wider]]);
    // The duplicate-brush union is 1 m³, but its specified brush sum is 2 m³.
    assert_eq!(regions.winner(Vec3::splat(0.5), |_| true), Some(1));
}

#[test]
fn teleport_overlap_winner_keeps_small_size_differences_then_map_order_ties() {
    let cube = axis_box(Vec3::ZERO, Vec3::ONE);
    let barely_wider = axis_box(Vec3::ZERO, Vec3::new(1.0002, 1.0, 1.0));
    let regions =
        TeleporterRegions::build(vec![vec![barely_wider], vec![cube.clone()], vec![cube]]);
    // These sizes collapse under the environment overlap quantization. The
    // teleporter's literal sum must prefer the smaller, later region instead.
    assert_eq!(regions.winner(Vec3::splat(0.5), |_| true), Some(1));
    // Exactly equal computed sums prefer the earlier eligible map entity.
    assert_eq!(
        regions.winner(Vec3::splat(0.5), |index| index != 0),
        Some(1)
    );
    assert_eq!(
        regions.winner(Vec3::splat(0.5), |index| index != 1),
        Some(2)
    );
}

#[test]
fn teleport_arrival_visits_all_overlapping_regions_once_and_filters_before_ranking() {
    let cube = axis_box(Vec3::ZERO, Vec3::ONE);
    let regions = TeleporterRegions::build(vec![
        vec![cube.clone(), cube.clone()],
        vec![axis_box(Vec3::splat(-1.0), Vec3::splat(2.0))],
        vec![cube],
        vec![axis_box(Vec3::splat(20.0), Vec3::splat(21.0))],
    ]);
    let point = Vec3::splat(0.5);
    let mut arrivals = Vec::new();
    regions.visit_containing(point, |index| arrivals.push(index));
    arrivals.sort_unstable();
    assert_eq!(arrivals, vec![0, 1, 2]);

    let mut eligibility_calls = [0; 4];
    let winner = regions.winner(point, |index| {
        eligibility_calls[index] += 1;
        index == 1
    });
    assert_eq!(winner, Some(1), "latched smaller regions do not compete");
    assert_eq!(eligibility_calls, [1, 1, 1, 0]);
    assert_eq!(regions.winner(point, |_| false), None);
}

#[test]
fn teleport_many_brush_regions_visit_once_with_deterministic_index_queries() {
    let source: Vec<_> = (0..40)
        .map(|index| {
            let extent = 1.0 + index as f32;
            let local = axis_box(Vec3::splat(-extent), Vec3::splat(extent));
            vec![
                local.clone(),
                local,
                axis_box(Vec3::splat(1000.0 + extent), Vec3::splat(1001.0 + extent)),
            ]
        })
        .collect();
    let first = TeleporterRegions::build(source.clone());
    let second = TeleporterRegions::build(source);
    let collect = |regions: &TeleporterRegions| {
        let mut hits = Vec::new();
        regions.visit_containing(Vec3::ZERO, |index| hits.push(index));
        hits
    };
    let mut hits = collect(&first);
    assert_eq!(hits, collect(&second), "rebuilds preserve visit order");
    hits.sort_unstable();
    assert_eq!(hits, (0..40).collect::<Vec<_>>());
    assert_eq!(first.winner(Vec3::ZERO, |_| true), Some(0));
    for index in 0..40 {
        assert!(first.contains(index, Vec3::ZERO));
        assert!(!first.contains(index, Vec3::splat(500.0)));
    }
}

#[test]
fn teleport_queries_reject_nonfinite_points_invalid_indices_and_empty_regions() {
    let regions = TeleporterRegions::build(vec![Vec::new(), vec![axis_box(Vec3::ZERO, Vec3::ONE)]]);
    assert_eq!(regions.len(), 2, "empty region preserves later map indices");
    assert!(!regions.contains(0, Vec3::splat(0.5)));
    assert!(!regions.contains(2, Vec3::splat(0.5)));
    assert!(!regions.contains(usize::MAX, Vec3::splat(0.5)));
    assert_eq!(regions.winner(Vec3::splat(0.5), |_| true), Some(1));
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for axis in 0..3 {
            let mut point = Vec3::splat(0.5);
            point[axis] = bad;
            assert!(!regions.contains(1, point));
            regions.visit_containing(point, |_| panic!("invalid point visited a region"));
            assert_eq!(
                regions.winner(point, |_| panic!("invalid point checked eligibility")),
                None
            );
        }
    }
    for empty in [TeleporterRegions::empty(), TeleporterRegions::default()] {
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        assert!(!empty.contains(0, Vec3::ZERO));
        assert_eq!(empty.winner(Vec3::ZERO, |_| true), None);
        empty.visit_containing(Vec3::ZERO, |_| panic!("empty set visited a region"));
    }
}

#[test]
fn teleport_overlap_winner_ties_offset_congruent_pads_by_map_order() {
    // Regression: volumes were clipped in world coordinates, so two copies of
    // one pad computed different rounding wherever they sat and float noise,
    // not map order, picked the winner.
    let pad = |origin: Vec3| {
        let mut pad = axis_box(origin, origin + Vec3::splat(2.0));
        // A slanted face makes clipping cut new vertices, where rounding lives.
        let normal = Vec3::new(1.0, 0.0, 0.375);
        pad.planes.push(LeafPlane {
            normal,
            distance: normal.dot(origin + Vec3::new(1.5, 0.0, 1.5)),
        });
        pad
    };
    // Dyadic coordinates keep both copies' stored planes exactly congruent.
    let offset = Vec3::new(0.75, 0.0, 0.5);
    for base in [
        Vec3::new(3.5, 0.0, 1.25),
        Vec3::new(517.25, 3.0, -211.5),
        Vec3::new(-1024.5, 40.25, 77.75),
        Vec3::new(4096.125, -96.5, 2048.25),
    ] {
        let point = base + Vec3::new(0.9, 1.0, 0.6);
        let (first, second) = (pad(base), pad(base + offset));
        for order in [
            vec![vec![first.clone()], vec![second.clone()]],
            vec![vec![second.clone()], vec![first.clone()]],
        ] {
            let regions = TeleporterRegions::build(order);
            assert!(regions.contains(0, point) && regions.contains(1, point));
            assert_eq!(
                regions.winner(point, |_| true),
                Some(0),
                "congruent pads at {base:?} tie, so the earlier map entity wins"
            );
        }
    }
}
