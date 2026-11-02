// Coverage proof tests: extracted and cut faces, crease cracks at and around τ, a far crack the parity weld hides, hinges, and order independence.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use glam::{DVec3, Vec3};

use super::test_fixtures::{
    Frame, Polygon, clip_polygon, closed_box, delta, extracted_room, face_polygons, hinge,
    lies_along, one_permit, permutations, prove, reorder, room_interior, spans_by_origin, tau,
    to_segment,
};
use super::{UnprovenSpan, prove_coverage};
use crate::map_data::{Face, TextureProjection};
use crate::partition::{OPEN_EDGE_SAMPLE_CAP, check_watertight};

fn polygon_normal(corners: &[Vec3]) -> DVec3 {
    let mut twice = DVec3::ZERO;
    for i in 0..corners.len() {
        twice += corners[i]
            .as_dvec3()
            .cross(corners[(i + 1) % corners.len()].as_dvec3());
    }
    twice
}

#[test]
fn coverage_proof_proves_an_extracted_room_with_cut_t_junctions_and_overlaps() {
    let (geometry, brushes, added) = extracted_room();
    assert!(added > 0, "the small pool edge cuts the room's long faces");
    let report = prove_coverage(&geometry, &brushes, &one_permit());
    assert!(
        report.unproven.is_empty(),
        "unproven spans on a closed extracted room: {:#?}",
        report.unproven
    );
    assert!(report.edges > 0);
    assert_eq!(report.edges_proven, report.edges);
}

// The stage's info line prints the unproven length: none reads "0.000", not
// the "-0.000" a float sum of nothing gives.
#[test]
fn coverage_proof_unproven_length_of_a_closed_box_is_positive_zero() {
    let report = prove(&closed_box(
        Frame::IDENTITY,
        DVec3::ZERO,
        DVec3::ONE,
        0.0,
        0,
    ));
    assert!(report.unproven.is_empty(), "{:#?}", report.unproven);
    assert_eq!(format!("{:.3}", report.unproven_length()), "0.000");
}

// The proof's soundness rests on every traced face winding counter-clockwise
// about its outward normal, through extraction, fan triangulation and the
// oversize-face cut.
#[test]
fn extracted_and_cut_triangles_wind_counter_clockwise_about_their_outward_normal() {
    let (geometry, _, added) = extracted_room();
    assert!(added > 0);
    let geom = &geometry.geometry;
    let mut checked = 0;
    for tri in geom.indices.chunks(3) {
        let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| &geom.vertices[i as usize]);
        let [pa, pb, pc] = [a, b, c].map(|v| Vec3::from(v.position).as_dvec3());
        let winding = (pb - pa).cross(pc - pa);
        if winding.length() < 1e-9 {
            continue;
        }
        let outward = Vec3::from(a.decode_normal()).as_dvec3();
        assert!(
            winding.normalize().dot(outward) > 0.99,
            "triangle {pa} {pb} {pc} winds against its normal {outward}"
        );
        checked += 1;
    }
    assert!(checked > 0);
}

#[test]
fn coverage_proof_reports_a_removed_strip_only_along_the_strip() {
    let (geometry, brushes, _) = extracted_room();
    let mut polygons = face_polygons(&geometry, &brushes);
    // The largest floor face loses a 2 cm strip along its longest edge.
    let floor = polygons
        .iter()
        .enumerate()
        .filter(|(_, p)| polygon_normal(&p.corners).normalize().y > 0.99)
        .max_by(|(_, a), (_, b)| {
            polygon_normal(&a.corners)
                .length()
                .total_cmp(&polygon_normal(&b.corners).length())
        })
        .map(|(i, _)| i)
        .expect("a floor face");
    let corners = polygons[floor].corners.clone();
    let normal = polygon_normal(&corners).normalize();
    let (a, b) = (0..corners.len())
        .map(|i| {
            (
                corners[i].as_dvec3(),
                corners[(i + 1) % corners.len()].as_dvec3(),
            )
        })
        .max_by(|(a0, b0), (a1, b1)| (*b0 - *a0).length().total_cmp(&(*b1 - *a1).length()))
        .expect("an edge");
    let along = (b - a).normalize();
    let length = (b - a).length();
    let centroid = corners.iter().map(|c| c.as_dvec3()).sum::<DVec3>() / corners.len() as f64;
    let mut inward = normal.cross(along);
    if (centroid - a).dot(inward) < 0.0 {
        inward = -inward;
    }
    const STRIP: f64 = 0.02;
    polygons[floor].corners = clip_polygon(&corners, a + inward * STRIP, inward);

    let report = prove(&polygons);
    assert!(!report.unproven.is_empty(), "the strip opens a hole");
    let slack = 1e-4;
    for span in &report.unproven {
        for &p in &span.endpoints {
            let v = p - a;
            assert!(v.dot(normal).abs() <= slack, "{span:?} leaves the floor");
            let across = v.dot(inward);
            assert!(
                (-slack..=STRIP + slack).contains(&across),
                "{span:?} lies {across} m across the strip"
            );
            let at = v.dot(along);
            assert!(
                (-STRIP - slack..=length + STRIP + slack).contains(&at),
                "{span:?} lies {at} m along the strip's edge"
            );
        }
    }
    // Both long sides of the strip are open: the neighbour's edge along the
    // removed one, and the clipped face's new edge.
    assert!(
        report.unproven_length() >= 2.0 * length - 1e-3,
        "unproven {} m against a {length} m strip",
        report.unproven_length()
    );
}

// P6: a crack wider than τ but narrower than 2τ is one the tracer blocks
// through its middle; the proof reads it as a hole.
#[test]
fn coverage_proof_proves_a_crease_crack_just_under_tau_and_not_just_over() {
    let frame = Frame::oblique(DVec3::new(2.0, 1.0, -3.0));
    let (min, max) = (DVec3::ZERO, DVec3::new(1.5, 1.0, 2.0));

    let under = prove(&closed_box(frame, min, max, 0.99 * tau(), 0));
    assert!(
        under.unproven.is_empty(),
        "a crack under τ is proven: {:#?}",
        under.unproven
    );

    let over = prove(&closed_box(frame, min, max, 1.01 * tau(), 0));
    // The four creases around the lifted top, each open from both sides.
    assert_eq!(over.unproven.len(), 8, "{:#?}", over.unproven);
    let creases = [
        (DVec3::new(0.0, 1.0, 0.0), DVec3::new(1.5, 1.0, 0.0)),
        (DVec3::new(1.5, 1.0, 0.0), DVec3::new(1.5, 1.0, 2.0)),
        (DVec3::new(1.5, 1.0, 2.0), DVec3::new(0.0, 1.0, 2.0)),
        (DVec3::new(0.0, 1.0, 2.0), DVec3::new(0.0, 1.0, 0.0)),
    ]
    .map(|(a, b)| (frame.point(a).as_dvec3(), frame.point(b).as_dvec3()));
    for span in &over.unproven {
        assert!(
            creases
                .iter()
                .any(|&(a, b)| lies_along(span, a, b, 2.0 * tau())),
            "{span:?} lies off every crease"
        );
    }
}

// A crack within τ at its quarter points, where the spike's classifier
// probed, and wider than τ between them.
#[test]
fn coverage_proof_reports_a_crack_wider_than_tau_between_its_quarter_points() {
    let frame = Frame::oblique(DVec3::new(-4.0, 0.5, 7.0));
    let (min, max) = (DVec3::ZERO, DVec3::new(2.0, 1.0, 1.0));
    let mut polygons = closed_box(frame, min, max, 0.0, 0);
    // The top face's edge over the +Z side gains a middle corner pulled in
    // by 1.5τ, a notch 0.75τ deep at the quarter points. The edge is not the
    // fan's first corner's, so the fan still tiles the notched loop.
    let (a, b) = (DVec3::new(0.0, 1.0, 1.0), DVec3::new(2.0, 1.0, 1.0));
    let middle = frame.point(DVec3::new(1.0, 1.0, 1.0 - 1.5 * tau()));
    let top = &mut polygons[3].corners;
    assert_eq!(
        (top[1], top[2]),
        (frame.point(a), frame.point(b)),
        "the top's second edge runs along the +Z side"
    );
    top.insert(2, middle);

    let report = prove(&polygons);
    assert!(!report.unproven.is_empty());
    let (a, b) = (frame.point(a).as_dvec3(), frame.point(b).as_dvec3());
    for span in &report.unproven {
        assert!(
            lies_along(span, a, b, 2.0 * tau()),
            "{span:?} off the crease"
        );
    }
    // Every point of the side's straight edge between the quarter points
    // lies in one of its unproven spans.
    let covered = |s: f64| {
        let p = a + (b - a) * s;
        report.unproven.iter().any(|span| {
            let [p0, p1] = span.endpoints;
            to_segment(p, p0, p1) <= 1e-9
        })
    };
    for step in 0..=20 {
        let s = 0.25 + 0.5 * f64::from(step) / 20.0;
        assert!(covered(s), "the crease is proven at {s} of its length");
    }
}

// Past a few hundred metres the parity weld's direction and anchor quanta
// put a crack's two edge lines on one key, so their coverage cancels.
#[test]
fn coverage_proof_finds_a_far_crack_the_watertightness_weld_hides() {
    let (start, length) = (320.0, 8.0);
    // Two lines through nearly the same anchor near the origin, apart by
    // 0.535–0.545 mm (just over τ) along the edges.
    let line = |x: f64, sign: f64| {
        let off = sign * (0.045e-3 + 0.45e-6 * x);
        DVec3::new(x, off, off).as_vec3()
    };
    let (a0, a1) = (line(start, -1.0), line(start + length, -1.0));
    let (b0, b1) = (line(start, 1.0), line(start + length, 1.0));
    let mid = start + 0.5 * length;
    // Face A spreads toward −y from its edge, face B toward +z: a crease.
    let ca = line(mid, -1.0) - Vec3::Y;
    let cb = line(mid, 1.0) + Vec3::Z;
    let triangles = [vec![a0, a1, ca], vec![b1, b0, cb]];

    let faces: Vec<Face> = triangles
        .iter()
        .enumerate()
        .map(|(brush, corners)| Face {
            vertices: corners.iter().map(|c| c.as_dvec3()).collect(),
            normal: polygon_normal(corners).normalize(),
            distance: 0.0,
            texture: "test".to_owned(),
            tex_projection: TextureProjection::default(),
            brush_index: brush,
        })
        .collect();
    let crack =
        |p: DVec3| p.x >= start && p.x <= start + length && p.y.abs() < 1e-3 && p.z.abs() < 1e-3;
    let parity = check_watertight(&faces);
    assert_eq!(
        parity.open_edge_count, 4,
        "only the triangles' outer edges read open"
    );
    assert!(parity.samples.iter().all(|edge| !crack(edge.midpoint)));

    let report = prove(
        &triangles
            .iter()
            .enumerate()
            .map(|(brush, corners)| Polygon {
                corners: corners.clone(),
                brush: Some(brush as u32),
            })
            .collect::<Vec<_>>(),
    );
    let on_crack: Vec<&UnprovenSpan> = report
        .unproven
        .iter()
        .filter(|span| span.endpoints.iter().all(|&p| crack(p)))
        .collect();
    assert_eq!(
        on_crack.len(),
        2,
        "both crack edges unproven: {:#?}",
        report.unproven
    );
    for span in on_crack {
        assert!((span.length - length).abs() < 1e-3);
    }
}

// P8: the four faces on a hinge cancel in pairs in every order.
#[test]
fn coverage_proof_balances_a_hinge_in_every_face_order() {
    let (polygons, slots, _) = hinge(Frame::oblique(DVec3::new(1.0, 2.0, 3.0)));
    for order in permutations(4) {
        let (faces, _) = reorder(&polygons, &slots, &order);
        let report = prove(&faces);
        assert!(
            report.unproven.is_empty(),
            "order {order:?}: {:#?}",
            report.unproven
        );
    }
}

// P8: a hinge missing a face, or two faces whose hinge edges run the same
// way, leaves an unproven span on the hinge itself, though every remaining
// edge there lies on another face; the same spans in every order.
#[test]
fn coverage_proof_reports_a_hinge_missing_one_face_or_two_running_the_same_way() {
    let frame = Frame::oblique(DVec3::new(1.0, 2.0, 3.0));
    let (polygons, slots, (h0, h1)) = hinge(frame);
    let hinge_direction = (h1 - h0).normalize();
    // Each hinge face's edge on the hinge: +1 when it runs h0 → h1.
    let runs = |polygon: &Polygon| {
        let c = &polygon.corners;
        (0..c.len())
            .map(|i| (c[i].as_dvec3(), c[(i + 1) % c.len()].as_dvec3()))
            .find(|&(p, q)| to_segment(p, h0, h1) < 1e-6 && to_segment(q, h0, h1) < 1e-6)
            .map(|(p, q)| (q - p).dot(hinge_direction).signum())
            .expect("a hinge face has a hinge edge")
    };

    let mut removals: Vec<Vec<usize>> = slots.iter().map(|&s| vec![s]).collect();
    let same_way: Vec<usize> = slots
        .iter()
        .copied()
        .filter(|&s| runs(&polygons[s]) > 0.0)
        .collect();
    assert_eq!(same_way.len(), 2, "two hinge edges run each way");
    assert_ne!(
        polygons[same_way[0]].brush, polygons[same_way[1]].brush,
        "one from each box"
    );
    removals.push(same_way);

    for removed in removals {
        let kept_slots: Vec<usize> = slots
            .iter()
            .copied()
            .filter(|s| !removed.contains(s))
            .collect();
        let mut expected: Option<Vec<UnprovenSpan>> = None;
        for order in permutations(kept_slots.len()) {
            let (reordered, origin) = reorder(&polygons, &kept_slots, &order);
            let (faces, origin): (Vec<Polygon>, Vec<usize>) = reordered
                .into_iter()
                .zip(origin)
                .filter(|(_, o)| !removed.contains(o))
                .unzip();
            let report = prove(&faces);
            let on_hinge = report
                .unproven
                .iter()
                .filter(|span| lies_along(span, h0, h1, 1e-6))
                .count();
            assert_eq!(
                on_hinge,
                removed.len(),
                "removing {removed:?}, order {order:?}: {:#?}",
                report.unproven
            );
            let spans = spans_by_origin(&report, &origin);
            match &expected {
                None => expected = Some(spans),
                Some(first) => assert_eq!(&spans, first, "removing {removed:?}, order {order:?}"),
            }
        }
    }
}

#[test]
fn coverage_proof_report_is_the_same_whatever_the_face_order() {
    let (geometry, brushes, _) = extracted_room();
    let mut polygons = face_polygons(&geometry, &brushes);
    // Open some holes so there is something to report: drop every third
    // face of the floor slab's top.
    let (low, _) = room_interior();
    let mut dropped = 0;
    polygons.retain(|p| {
        let floor = p
            .corners
            .iter()
            .all(|c| (f64::from(c.y) - low.y).abs() < 1e-4);
        dropped += usize::from(floor);
        !(floor && dropped % 3 == 0)
    });
    let first = prove(&polygons);
    assert!(!first.unproven.is_empty());

    // A fixed shuffle: a multiplicative step coprime with the face count.
    let n = polygons.len();
    let step = (1..n)
        .rev()
        .find(|s| gcd(*s, n) == 1 && *s > n / 3)
        .unwrap_or(1);
    let origin: Vec<usize> = (0..n).map(|k| (k * step + 7) % n).collect();
    let shuffled: Vec<Polygon> = origin.iter().map(|&i| polygons[i].clone()).collect();
    let second = prove(&shuffled);
    assert_eq!(second.edges, first.edges);
    assert_eq!(second.edges_proven, first.edges_proven);
    assert_eq!(spans_by_origin(&second, &origin), first.unproven);
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

#[test]
fn coverage_proof_samples_stop_at_the_watertightness_sample_cap() {
    let mut polygons = Vec::new();
    for k in 0..4 {
        let mut open = closed_box(
            Frame::IDENTITY,
            DVec3::new(3.0 * f64::from(k), 0.0, 0.0),
            DVec3::new(3.0 * f64::from(k) + 1.0, 1.0, 1.0),
            0.0,
            k,
        );
        open.remove(3);
        polygons.extend(open);
    }
    let report = prove(&polygons);
    assert_eq!(report.unproven.len(), 16, "each open top leaves four rims");
    assert!(report.unproven.len() > OPEN_EDGE_SAMPLE_CAP);
    assert_eq!(report.samples().len(), OPEN_EDGE_SAMPLE_CAP);
    let edge = report.samples()[0].open_edge().expect("a brush");
    assert_eq!(edge.midpoint, report.samples()[0].midpoint);
}

/// A box `[0, 2] × [0, 1] × [0, 1]` whose top is two faces split at x = 1,
/// the right one's split edge moved to x = 1 + `shift`: a gap across the
/// top for a positive shift, an overlap for a negative one. The sides'
/// long top edges meet the split as T-junctions.
fn split_top_box(frame: Frame, shift: f64) -> Vec<Polygon> {
    let mut polygons = closed_box(frame, DVec3::ZERO, DVec3::new(2.0, 1.0, 1.0), 0.0, 0);
    let p = |x: f64, z: f64| frame.point(DVec3::new(x, 1.0, z));
    let split = 1.0 + shift;
    polygons[3] = Polygon {
        corners: vec![p(0.0, 0.0), p(0.0, 1.0), p(1.0, 1.0), p(1.0, 0.0)],
        brush: Some(0),
    };
    polygons.push(Polygon {
        corners: vec![p(split, 0.0), p(split, 1.0), p(2.0, 1.0), p(2.0, 0.0)],
        brush: Some(1),
    });
    polygons
}

// Edge ends meeting at one point land apart along their line, by rounding
// or a crack's ends: a gap or an overlap. Within δ the copies weld and the
// proof reads them closed. A gap within τ is closed by a stand-in. An
// overlap past δ is a coplanar same-winding double cover, and reads as a
// hole, a named cost; past τ so does a gap.
#[test]
fn coverage_proof_proves_split_ends_welded_within_delta_and_gaps_within_tau() {
    let frame = Frame::oblique(DVec3::new(0.5, -1.0, 2.0));
    for shift in [0.5 * delta(), -0.5 * delta(), 0.5 * tau()] {
        let report = prove(&split_top_box(frame, shift));
        assert!(
            report.unproven.is_empty(),
            "split ends {shift} m apart: {:#?}",
            report.unproven
        );
    }
    for shift in [-0.5 * tau(), 3.0 * tau(), -3.0 * tau()] {
        let report = prove(&split_top_box(frame, shift));
        assert!(
            !report.unproven.is_empty(),
            "split ends {shift} m apart read closed"
        );
        // Every span lies on a split edge, or on a side's rim between them.
        for span in &report.unproven {
            let on_split = [1.0, 1.0 + shift].iter().any(|&x| {
                lies_along(
                    span,
                    frame.point(DVec3::new(x, 1.0, 0.0)).as_dvec3(),
                    frame.point(DVec3::new(x, 1.0, 1.0)).as_dvec3(),
                    1e-6,
                )
            });
            let on_rim = [0.0, 1.0].iter().any(|&z| {
                lies_along(
                    span,
                    frame
                        .point(DVec3::new(1.0 + shift.min(0.0), 1.0, z))
                        .as_dvec3(),
                    frame
                        .point(DVec3::new(1.0 + shift.max(0.0), 1.0, z))
                        .as_dvec3(),
                    1e-6,
                )
            });
            assert!(on_split || on_rim, "shift {shift}: {span:?} off the split");
        }
    }
}

// The rounding margin grows with the geometry's extent. Past about 8.8e9 m
// it leaves nothing of τ, and a non-finite coordinate (NaN included) has no
// extent to bound it: either way no distance proves anything, so even a
// closed box reads every edge unproven. At 1e9 m the margin still fits.
#[test]
fn coverage_proof_fails_closed_where_rounding_can_reach_tau() {
    let far_box = |at: f64, size: f64| {
        closed_box(
            Frame {
                rotation: glam::DQuat::IDENTITY,
                offset: DVec3::splat(at),
            },
            DVec3::ZERO,
            DVec3::splat(size),
            0.0,
            0,
        )
    };
    let with_corner = |corner: Vec3| {
        let mut polygons = closed_box(Frame::IDENTITY, DVec3::ZERO, DVec3::ONE, 0.0, 0);
        polygons.push(Polygon {
            corners: vec![Vec3::new(3.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 0.0), corner],
            brush: Some(1),
        });
        polygons
    };

    let report = prove(&far_box(1e9, 128.0));
    assert!(report.unproven.is_empty(), "{:#?}", report.unproven);

    for (case, polygons) in [
        ("box at 1e10 m", far_box(1e10, 4096.0)),
        (
            "infinite corner",
            with_corner(Vec3::new(3.0, f32::INFINITY, 0.0)),
        ),
        ("NaN corner", with_corner(Vec3::new(3.0, f32::NAN, 0.0))),
    ] {
        let report = prove(&polygons);
        assert!(report.edges > 0, "{case}");
        assert_eq!(report.edges_proven, 0, "{case}");
        assert_eq!(report.unproven.len(), report.edges, "{case}");
    }
}
