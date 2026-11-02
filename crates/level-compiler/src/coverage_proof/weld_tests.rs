// Coverage proof tests: the weld at corners, cracks and slivers — what it merges, what it refuses, and that it never measures.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use glam::DVec3;

use super::test_fixtures::{
    Frame, Polygon, beam_corner, closed_box, delta, facing, hinge, lies_along, permutations,
    polygon_geometry, prove, prove_unwelded, reorder, spans_by_origin, tau, unproven_along,
    unproven_near, weld_clusters,
};
use super::weld::Verdict;
use super::{CoverageReport, UnprovenSpan, WELD_RADIUS_METERS, prove_coverage};
use crate::geometry::GeometryResult;
use crate::ray_traversal::TRACER_TOLERANCE_METERS;

fn frame() -> Frame {
    Frame::oblique(DVec3::new(-3.0, 4.0, 1.0))
}

/// Some unproven span lies within `reach` of `corner`, both ends.
fn unproven_at(report: &CoverageReport, corner: DVec3, reach: f64) -> bool {
    report.unproven.iter().any(|span| {
        span.length < reach
            && span
                .endpoints
                .iter()
                .all(|&p| (p - corner).length() < reach)
    })
}

#[test]
fn weld_radius_is_at_most_a_quarter_of_the_tracer_tolerance() {
    assert!(WELD_RADIUS_METERS <= f64::from(TRACER_TOLERANCE_METERS) / 4.0);
}

// The movement-feel corners. A wall line meets a beam's flat bottom and
// sloped face, and the wall's, the bottom's and the sloped face's copies of
// the corner land micrometres apart along the line, leaving an overlap.
// The copies weld into one vertex and the overlap is gone.
#[test]
fn coverage_proof_proves_an_overlap_at_a_corner_whose_copies_weld() {
    let beam = beam_corner(frame(), [-5e-6, 0.0, 5e-6], None);
    let report = prove(&beam.polygons);
    assert!(
        report.unproven.is_empty(),
        "a closed corner: {:#?}",
        report.unproven
    );
}

// Without the sloped face, the corner's copies still weld, and the face
// missing from the corner is open along each of its rims.
#[test]
fn coverage_proof_reports_a_corner_missing_its_closing_face() {
    let beam = beam_corner(frame(), [-5e-6, 0.0, 5e-6], None);
    let mut polygons = beam.polygons;
    polygons.remove(beam.slots[2]);
    let report = prove(&polygons);
    for rim in beam.sloped_rims {
        let length = (rim.1 - rim.0).length();
        let along = unproven_near(&report, rim, 2.0 * delta());
        assert!(
            along >= length - 2.0 * delta(),
            "{along} m of a {length} m rim unproven"
        );
    }
    assert!(
        report.unproven.iter().any(|span| span
            .endpoints
            .iter()
            .any(|&p| (p - beam.corner).length() < 1e-4)),
        "no unproven span reaches the corner: {:#?}",
        report.unproven
    );
}

// Negative control: with the weld off, the same closed corner's copies
// leave their overlap unproven. Nothing else closes it.
#[test]
fn coverage_proof_without_the_weld_reports_the_overlap_at_a_closed_corner() {
    let beam = beam_corner(frame(), [-5e-6, 0.0, 5e-6], None);
    let report = prove_unwelded(&beam.polygons);
    assert!(
        unproven_at(&report, beam.corner, 1e-4),
        "no unproven overlap at the corner: {:#?}",
        report.unproven
    );
}

// Copies just under δ apart weld, and just over δ apart do not. Near
// the origin, so `f32` storage keeps the distance well inside the margin.
#[test]
fn coverage_proof_welds_copies_just_under_delta_apart_and_not_just_over() {
    let near = Frame::oblique(DVec3::ZERO);
    for (scale, proven) in [(1.0 - 1e-3, true), (1.0 + 1e-3, false)] {
        let apart = delta() * scale;
        let beam = beam_corner(near, [-0.5 * apart, 0.0, 0.5 * apart], None);
        // Each face's corner nearest P is its copy.
        let stored = |slot: usize| {
            beam.polygons[slot]
                .corners
                .iter()
                .map(|c| c.as_dvec3())
                .min_by(|p, q| {
                    (*p - beam.corner)
                        .length()
                        .total_cmp(&(*q - beam.corner).length())
                })
                .expect("a corner")
        };
        let copies = [stored(beam.slots[0]), stored(beam.slots[1])];
        let distance = (copies[0] - copies[1]).length();
        assert_eq!(
            distance <= delta(),
            proven,
            "stored copies {distance} m apart"
        );
        let report = prove(&beam.polygons);
        assert_eq!(
            report.unproven.is_empty(),
            proven,
            "copies {apart} m apart: {:#?}",
            report.unproven
        );
    }
}

// Three copies at c, c + 0.6δ and c + 1.2δ link pair by pair into one
// cluster 1.2δ wide. It is refused, so the overlap stays unproven, the same
// in every order of the three faces.
#[test]
fn coverage_proof_refuses_a_chain_of_copies_wider_than_delta_in_every_order() {
    let beam = beam_corner(frame(), [0.0, 0.6 * delta(), 1.2 * delta()], None);
    assert!(
        weld_clusters(&beam.polygons)
            .iter()
            .any(|cluster| cluster.verdict == Verdict::TooWide && cluster.members.len() == 3),
        "the three copies form one refused cluster"
    );
    let mut expected: Option<Vec<UnprovenSpan>> = None;
    for order in permutations(3) {
        let (faces, origin) = reorder(&beam.polygons, &beam.slots, &order);
        let report = prove(&faces);
        assert!(
            unproven_at(&report, beam.corner, 2.0 * delta()),
            "order {order:?}: {:#?}",
            report.unproven
        );
        let spans = spans_by_origin(&report, &origin);
        match &expected {
            None => expected = Some(spans),
            Some(first) => assert_eq!(&spans, first, "order {order:?}"),
        }
    }
}

/// A box `[0, 1.5] × [0, 1] × [0, 2]` whose top is lifted `lift`, opening
/// a crease crack of that width round it. Each side face is split at its
/// middle across the crease, the two halves' copies of the split corners
/// `copy` apart along the line: a gap on the ±X sides, an overlap on the
/// ±Z sides.
fn split_crease_box(lift: f64, copy: f64) -> Vec<Polygon> {
    let max = DVec3::new(1.5, 1.0, 2.0);
    let mut polygons = closed_box(frame(), DVec3::ZERO, max, lift, 0);
    // Order −X, +X, −Y, +Y, −Z, +Z: split the four sides, keep the rest.
    let sides = [
        (0, 2, 0.0, DVec3::NEG_X),
        (1, 2, max.x, DVec3::X),
        (4, 0, 0.0, DVec3::NEG_Z),
        (5, 0, max.z, DVec3::Z),
    ];
    let mut split = Vec::new();
    for (slot, along, fixed, outward) in sides {
        let normal_axis = if along == 2 { 0 } else { 2 };
        let middle = max[along] * 0.5;
        // A gap where the halves run along z, an overlap where along x.
        let (low_end, high_start) = if along == 2 {
            (middle - 0.5 * copy, middle + 0.5 * copy)
        } else {
            (middle + 0.5 * copy, middle - 0.5 * copy)
        };
        for (lo, hi) in [(0.0, low_end), (high_start, max[along])] {
            let point = |a: f64, y: f64| {
                let mut p = DVec3::ZERO;
                p[normal_axis] = fixed;
                p[along] = a;
                p.y = y;
                p
            };
            split.push(facing(
                frame(),
                vec![
                    point(lo, 0.0),
                    point(hi, 0.0),
                    point(hi, 1.0),
                    point(lo, 1.0),
                ],
                outward,
                0,
            ));
        }
        polygons[slot].corners.clear();
    }
    polygons.retain(|p| !p.corners.is_empty());
    polygons.extend(split);
    polygons
}

// A crease crack just under τ wide whose crack edges are split mid-way,
// their copies δ/2 apart along the line. The copies weld and the crack is
// proven: distances read stored geometry with the full τ, no δ off it.
#[test]
fn coverage_proof_proves_a_crease_crack_under_tau_with_welded_copies() {
    let report = prove(&split_crease_box(0.99 * tau(), 0.5 * delta()));
    assert!(
        report.unproven.is_empty(),
        "a crack under τ with welded copies: {:#?}",
        report.unproven
    );
}

// The same crack just over τ is a hole along its creases.
#[test]
fn coverage_proof_reports_a_crease_crack_over_tau_with_welded_copies() {
    let report = prove(&split_crease_box(1.01 * tau(), 0.5 * delta()));
    let max = DVec3::new(1.5, 1.0, 2.0);
    let corners = [
        DVec3::new(0.0, 1.0, 0.0),
        DVec3::new(max.x, 1.0, 0.0),
        DVec3::new(max.x, 1.0, max.z),
        DVec3::new(0.0, 1.0, max.z),
    ];
    for k in 0..4 {
        let crease = (
            frame().point(corners[k]).as_dvec3(),
            frame().point(corners[(k + 1) % 4]).as_dvec3(),
        );
        let length = (crease.1 - crease.0).length();
        let along = unproven_near(&report, crease, 2.0 * tau());
        assert!(
            along >= 2.0 * (length - 2.0 * delta()),
            "{along} m unproven on a {length} m crease, both sides"
        );
    }
}

// A gap τ + 0.9δ wide between two edges, each of whose corners has a
// copy 0.95δ toward the other edge. The copies weld and represent their
// clusters, so the welded edges lie τ − δ apart; the stored ones do not,
// and the gap is a hole. Axis-aligned across x = 0, so every copy, nearer
// zero, has the smaller bits.
#[test]
fn coverage_proof_measures_stored_corners_not_their_weld_representatives() {
    let gap = tau() + 0.9 * delta();
    let length = 0.01;
    let half = 0.5 * gap;
    let quad = |x0: f64, x1: f64, brush: u32| Polygon {
        corners: [
            DVec3::new(x0, 0.0, 0.0),
            DVec3::new(x1, 0.0, 0.0),
            DVec3::new(x1, 0.0, length),
            DVec3::new(x0, 0.0, length),
        ]
        .iter()
        .map(|p| p.as_vec3())
        .collect(),
        brush: Some(brush),
    };
    let mut polygons = vec![quad(half, 0.1, 0), quad(-0.1, -half, 1)];
    let mut brush = 2;
    for x in [half - 0.95 * delta(), -half + 0.95 * delta()] {
        for z in [0.0, length] {
            let copy = DVec3::new(x, 0.0, z);
            let below = copy - DVec3::Y * 0.01;
            polygons.push(Polygon {
                corners: [copy, below, below + DVec3::Z * 0.01]
                    .iter()
                    .map(|p| p.as_vec3())
                    .collect(),
                brush: Some(brush),
            });
            brush += 1;
        }
    }
    let welded: Vec<_> = weld_clusters(&polygons)
        .into_iter()
        .filter(|cluster| cluster.verdict == Verdict::Welded)
        .collect();
    assert_eq!(welded.len(), 4, "each corner welds to its copy");
    for cluster in &welded {
        let x = f64::from(f32::from_bits(cluster.members[0][0]).abs());
        assert!(x < half - 0.5 * delta(), "the copy represents its cluster");
    }
    let report = prove(&polygons);
    for x in [half, -half] {
        let edge = (DVec3::new(x, 0.0, 0.0), DVec3::new(x, 0.0, length));
        let along = unproven_along(&report, edge);
        assert!(
            along >= length - 2.0 * delta(),
            "{along} m of the {length} m edge at x = {x} unproven"
        );
    }
}

// A cost: across a crack 3δ wide, one side is two same-way edges whose
// ends overlap 0.2δ along the line, their corners 1.5δ apart across it, too
// far to weld. The overlap holds two edges against one, and the cap lets
// no stand-in lift the one, so it reads unproven; the rest of the crack is
// proven.
#[test]
fn coverage_proof_reports_the_overlapping_ends_of_a_crack_wider_than_delta() {
    let (width, overlap, length) = (3.0 * delta(), 0.2 * delta(), 0.01);
    let (mid, step) = (0.5 * length, 1.5 * delta());
    // A1 and A2 spread toward −y in z = 0 and run +x; B spreads toward +z
    // in y = 3δ and runs −x.
    let a1 = [DVec3::ZERO, DVec3::X * (mid + 0.5 * overlap)];
    let a2 = [
        DVec3::new(mid - 0.5 * overlap, step, 0.0),
        DVec3::new(length, step, 0.0),
    ];
    let b = [DVec3::new(0.0, width, 0.0), DVec3::new(length, width, 0.0)];
    let polygons = [
        vec![a1[0], a1[1], DVec3::new(0.25 * length, -0.01, 0.0)],
        vec![a2[0], a2[1], DVec3::new(0.75 * length, step - 0.01, 0.0)],
        vec![b[1], b[0], DVec3::new(mid, width, 0.01)],
    ]
    .into_iter()
    .enumerate()
    .map(|(brush, corners)| Polygon {
        corners: corners.iter().map(|&p| frame().point(p)).collect(),
        brush: Some(brush as u32),
    })
    .collect::<Vec<_>>();
    let report = prove(&polygons);
    let world = |p: DVec3| frame().point(p).as_dvec3();
    let lines = [a1, a2, b].map(|[p, q]| (world(p), world(q)));
    let on_crack: Vec<&UnprovenSpan> = report
        .unproven
        .iter()
        .filter(|span| lines.iter().any(|&(p, q)| lies_along(span, p, q, 1e-6)))
        .collect();
    let total: f64 = on_crack.iter().map(|span| span.length).sum();
    assert!(
        (total - overlap).abs() < 1e-6,
        "{total} m unproven on the crack against its {overlap} m overlap: {on_crack:#?}"
    );
    let middle = world(DVec3::new(mid, 0.5 * step, 0.0));
    for span in on_crack {
        assert!(
            span.endpoints
                .iter()
                .all(|&p| (p - middle).length() <= step + overlap),
            "{span:?} lies off the overlap"
        );
    }
}

/// `geometry` with its vertex buffer reversed and its indices remapped.
fn reversed_vertices(geometry: &GeometryResult) -> GeometryResult {
    let mut out = geometry.clone();
    let count = out.geometry.vertices.len() as u32;
    out.geometry.vertices.reverse();
    for index in &mut out.geometry.indices {
        *index = count - 1 - *index;
    }
    out
}

// The hinge's four faces in all 24 orders, its vertex buffer in
// two orders, and the whole hinge translated far off the origin: the same
// verdict and the same spans, up to the translation, intact and with one
// face removed.
#[test]
fn coverage_proof_hinge_verdict_ignores_face_order_vertex_order_and_translation() {
    let base = DVec3::new(1.0, 2.0, 3.0);
    let shift = DVec3::new(1000.123, -517.9, 33.3);
    for removed in [None, Some(6)] {
        let mut expected: Option<Vec<UnprovenSpan>> = None;
        for offset in [base, base + shift] {
            let (polygons, slots, _) = hinge(Frame::oblique(offset));
            for order in permutations(4) {
                let (faces, origin) = reorder(&polygons, &slots, &order);
                let (faces, origin): (Vec<Polygon>, Vec<usize>) = faces
                    .into_iter()
                    .zip(origin)
                    .filter(|&(_, o)| Some(o) != removed)
                    .unzip();
                let (geometry, brushes) = polygon_geometry(&faces);
                for geometry in [reversed_vertices(&geometry), geometry] {
                    let report = prove_coverage(&geometry, &brushes);
                    assert_eq!(report.unproven.is_empty(), removed.is_none());
                    let mut spans = spans_by_origin(&report, &origin);
                    for span in &mut spans {
                        span.endpoints = span.endpoints.map(|p| p - (offset - base));
                    }
                    let Some(first) = &expected else {
                        expected = Some(spans);
                        continue;
                    };
                    assert_eq!(spans.len(), first.len(), "order {order:?}");
                    for (span, first) in spans.iter().zip(first) {
                        assert_eq!(span.face, first.face, "order {order:?}");
                        for (p, q) in span.endpoints.iter().zip(&first.endpoints) {
                            assert!((*p - *q).length() < 1e-4, "{span:?} vs {first:?}");
                        }
                    }
                }
            }
        }
    }
}

/// A box `[0, 1]³` whose top is three faces split along z: A to x = 0.5,
/// a sliver 0.5δ wide, and B. Returns the faces, A's slot and the sliver's
/// edge beside A.
fn sliver_top() -> (Vec<Polygon>, usize, (DVec3, DVec3)) {
    let mut polygons = closed_box(frame(), DVec3::ZERO, DVec3::ONE, 0.0, 0);
    let (x0, x1) = (0.5, 0.5 + 0.5 * delta());
    let strip = |a: f64, b: f64, brush: u32| {
        facing(
            frame(),
            vec![
                DVec3::new(a, 1.0, 0.0),
                DVec3::new(a, 1.0, 1.0),
                DVec3::new(b, 1.0, 1.0),
                DVec3::new(b, 1.0, 0.0),
            ],
            DVec3::Y,
            brush,
        )
    };
    // Order −X, +X, −Y, +Y: the top is slot 3.
    polygons[3] = strip(0.0, x0, 0);
    polygons.push(strip(x0, x1, 1));
    polygons.push(strip(x1, 1.0, 2));
    let edge = (
        frame().point(DVec3::new(x0, 1.0, 0.0)).as_dvec3(),
        frame().point(DVec3::new(x0, 1.0, 1.0)).as_dvec3(),
    );
    (polygons, 3, edge)
}

// A sliver 0.5δ wide between two neighbours proves; without one
// neighbour, the stretch beside it is open along its metre.
#[test]
fn coverage_proof_proves_a_sliver_between_neighbours_and_not_beside_a_missing_one() {
    let (polygons, a, edge) = sliver_top();
    let intact = prove(&polygons);
    assert!(intact.unproven.is_empty(), "{:#?}", intact.unproven);
    let mut open = polygons;
    open.remove(a);
    let report = prove(&open);
    let along = unproven_near(&report, edge, delta());
    assert!(
        along >= 1.0 - 2.0 * delta(),
        "{along} m beside the sliver unproven"
    );
}

// Two closed boxes corner to corner, their corners 0.5δ apart, weld
// there and prove; the second box's far face, a metre away, dropped, is
// open along all four rims.
#[test]
fn coverage_proof_welds_boxes_corner_to_corner_and_finds_a_face_dropped_a_metre_away() {
    let near = DVec3::ONE + DVec3::splat(0.5 * delta() / 3f64.sqrt());
    let far = DVec3::splat(2.0);
    let mut polygons = closed_box(frame(), DVec3::ZERO, DVec3::ONE, 0.0, 0);
    polygons.extend(closed_box(frame(), near, far, 0.0, 1));
    assert!(
        weld_clusters(&polygons)
            .iter()
            .any(|cluster| cluster.verdict == Verdict::Welded),
        "the corners weld"
    );
    let intact = prove(&polygons);
    assert!(intact.unproven.is_empty(), "{:#?}", intact.unproven);
    // Order −X, +X, …: the second box's +X face, at x = 2, is slot 6 + 1.
    let mut open = polygons;
    open.remove(7);
    let report = prove(&open);
    let rim_corners = [
        DVec3::new(2.0, near.y, near.z),
        DVec3::new(2.0, far.y, near.z),
        DVec3::new(2.0, far.y, far.z),
        DVec3::new(2.0, near.y, far.z),
    ];
    for k in 0..4 {
        let rim = (
            frame().point(rim_corners[k]).as_dvec3(),
            frame().point(rim_corners[(k + 1) % 4]).as_dvec3(),
        );
        let length = (rim.1 - rim.0).length();
        let along = unproven_along(&report, rim);
        assert!(
            along >= length - 2.0 * delta(),
            "{along} m of a {length} m rim"
        );
    }
}

// A cost: the wall's upper face has a T-junction corner of its own
// within δ of its corner copy, beside the bottom's copy. The cluster holds
// two corners of one triangle, so it is refused, and the overlap stays
// unproven.
#[test]
fn coverage_proof_refuses_a_cluster_holding_a_face_own_t_junction() {
    let beam = beam_corner(frame(), [0.0, 0.0, 0.3 * delta()], Some(0.6 * delta()));
    assert!(
        weld_clusters(&beam.polygons)
            .iter()
            .any(|cluster| cluster.verdict == Verdict::SharesTriangle),
        "the corner's cluster is refused"
    );
    let report = prove(&beam.polygons);
    assert!(
        unproven_at(&report, beam.corner, 1e-4),
        "the overlap reads unproven: {:#?}",
        report.unproven
    );
}
