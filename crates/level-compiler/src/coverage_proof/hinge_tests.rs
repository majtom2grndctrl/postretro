// Coverage proof tests: hinges and boxes missing faces beside chains, strips, stagger and folds that a hinge or box must still report as unproven.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use glam::DVec3;

use super::test_fixtures::{
    Frame, Polygon, assert_hinge_hole_found, axis_skew_hinge, bottom_strips, box_face,
    chained_hinge, closed_box, delta, facing, prove, prove_with, stations, tau, top_strips,
    unproven_along, unproven_near,
};
use super::weld::WeldRule;

fn frame() -> Frame {
    Frame::oblique(DVec3::new(1.0, 2.0, 3.0))
}

// A hinge missing a face reads unproven even when its cluster axis lies
// across it. One 0.7 mm edge merges the hinge into a cluster whose axis lies
// across the hinge, so every hinge edge projects under τ, and the whole
// metre must still read unproven. Slack is 2δ, not 2τ: a stand-in's reach is
// measured along the edge, so a sliver cannot partner the hinge's first
// 0.37 mm from more than τ past its end.
#[test]
fn coverage_proof_reports_a_hinge_missing_a_face_whose_cluster_axis_lies_across_it() {
    let (polygons, dropped, line) = axis_skew_hinge(frame());
    assert_hinge_hole_found(polygons, dropped, line, 2.0 * delta());
}

// A chain of sub-τ edges shares no single opposite edge. A face edge broken
// into 0.4 mm links, each shorter than τ, cannot all pair with the one
// opposite edge left on a hinge missing a face; each link needs a partner
// of its own.
#[test]
fn coverage_proof_reports_a_hinge_missing_a_face_beside_a_chain_of_sub_tau_edges() {
    // Brush 0's top, one loop with a collinear corner every 0.4 mm on the
    // hinge, triangulated from a corner off it.
    let mut top = vec![DVec3::new(-1.0, 0.0, 0.0)];
    top.extend((0..=2500).map(|k| DVec3::new(0.0, f64::from(k) / 2500.0, 0.0)));
    top.push(DVec3::new(-1.0, 1.0, 0.0));
    let bottom = bottom_strips(&[0.0, 1.0]);
    let (polygons, dropped, line) = chained_hinge(frame(), 1.0, vec![top], bottom);
    assert_hinge_hole_found(polygons, dropped, line, 2.0 * delta());
}

/// The two same-way faces on a 2 cm hinge, tiled into strips `width` wide,
/// the bottom's stations `stagger` past the top's. Strip joints a few
/// tenths of a millimetre apart link into one cluster, so the cost grows
/// with the strip count squared.
fn staggered_hinge(width: f64, stagger: f64) -> (Vec<Polygon>, usize, (DVec3, DVec3)) {
    const LENGTH: f64 = 0.02;
    let top = top_strips(&stations(LENGTH, width, width));
    let bottom = bottom_strips(&stations(LENGTH, stagger, width));
    chained_hinge(frame(), LENGTH, top, bottom)
}

// Strips staggered under 1 mm read unproven: both same-way faces on a hinge
// tiled into 0.4 mm strips, staggered by 0.2 mm, so every piece holds two
// same-way edges whose ends overlap by under τ.
#[test]
fn coverage_proof_reports_a_hinge_missing_a_face_beside_staggered_sub_tau_strips() {
    let (polygons, dropped, line) = staggered_hinge(0.0004, 0.0002);
    assert_hinge_hole_found(polygons, dropped, line, 2.0 * delta());
}

// Cap boxes cannot hide a missing face: 0.98 mm strips staggered by half
// their width, with a tiny closed box per stagger whose face runs through
// the hinge. The boxes close every corner at the staggers, and the hinge
// missing a face still reads unproven.
#[test]
fn coverage_proof_reports_a_hinge_missing_a_face_beside_strips_and_cap_boxes() {
    const WIDTH: f64 = 0.00098;
    let (mut polygons, dropped, line) = staggered_hinge(WIDTH, 0.5 * WIDTH);
    let (a, t, b) = (0.0011, 0.00005, 0.0005);
    let mut brush = 2;
    for c in stations(0.02, WIDTH, WIDTH) {
        for (lo, hi, through) in [
            (c + 0.25 * WIDTH - t, c + 0.25 * WIDTH, true),
            (c + 0.75 * WIDTH, c + 0.75 * WIDTH + t, false),
        ] {
            if hi >= 0.02 {
                continue;
            }
            let mut cap = closed_box(
                frame(),
                DVec3::new(-a, lo, 0.0),
                DVec3::new(a, hi, b),
                0.0,
                brush,
            );
            // Order −X, +X, −Y, +Y, …: the face through the hinge carries
            // the hinge point as a corner of its own.
            let (slot, y, outward) = if through {
                (3, hi, DVec3::Y)
            } else {
                (2, lo, DVec3::NEG_Y)
            };
            cap[slot] = facing(
                frame(),
                vec![
                    DVec3::new(a, y, b),
                    DVec3::new(a, y, 0.0),
                    DVec3::new(0.0, y, 0.0),
                    DVec3::new(-a, y, 0.0),
                    DVec3::new(-a, y, b),
                ],
                outward,
                brush,
            );
            polygons.extend(cap);
            brush += 1;
        }
    }
    assert_hinge_hole_found(polygons, dropped, line, 2.0 * delta());
}

// Strips staggered under δ, wider than 2δ, so each stagger's copies
// weld and the strips' ends coincide. Edge counts, not stagger size, find
// the face missing from the hinge.
#[test]
fn coverage_proof_reports_a_hinge_missing_a_face_beside_strips_whose_stagger_welds() {
    let width = 0.0004;
    assert!(width > 2.0 * delta());
    let (polygons, dropped, line) = staggered_hinge(width, width + 0.8 * delta());
    assert_hinge_hole_found(polygons, dropped, line, 2.0 * delta());
}

/// The box [0, s]³ (brush 1) with a neighbour box hinged on each of the
/// eight rims of its x = 0 and x = s faces. Its four other faces, and each
/// neighbour's face on the hinge beside them, are strips `width` wide; the
/// neighbours' stations start `first` in. Returns the faces, the slots of
/// the x = 0 and x = s faces, and the eight rims.
fn hinged_box(width: f64, first: f64) -> (Vec<Polygon>, [usize; 2], Vec<(DVec3, DVec3)>) {
    const S: f64 = 0.02;
    let own = stations(S, width, width);
    let theirs = stations(S, first, width);
    let core = (DVec3::ZERO, DVec3::splat(S));
    let mut polygons = Vec::new();
    let mut slots = [0; 2];
    for (k, high) in [false, true].into_iter().enumerate() {
        slots[k] = polygons.len();
        polygons.extend(box_face(frame(), core, (0, high), None, 1));
    }
    for (axis, across) in [(1, 2), (2, 1)] {
        for high in [false, true] {
            polygons.extend(box_face(
                frame(),
                core,
                (axis, high),
                Some((across, &own)),
                1,
            ));
        }
    }
    let mut rims = Vec::new();
    let mut brush = 2;
    for x_high in [false, true] {
        for (axis, along) in [(2, 1), (1, 2)] {
            for high in [false, true] {
                // The neighbour lies past the rim on both the x side and
                // this face's side, spanning the rim's length.
                let mut min = DVec3::ZERO;
                let mut max = DVec3::splat(S);
                let past = |high: bool| if high { (S, 2.0 * S) } else { (-S, 0.0) };
                (min.x, max.x) = past(x_high);
                (min[axis], max[axis]) = past(high);
                let bounds = (min, max);
                for face_axis in 0..3 {
                    for face_high in [false, true] {
                        let toward_core = (face_axis == axis && face_high != high)
                            .then_some((along, theirs.as_slice()));
                        polygons.extend(box_face(
                            frame(),
                            bounds,
                            (face_axis, face_high),
                            toward_core,
                            brush,
                        ));
                    }
                }
                brush += 1;
                let mut a = DVec3::ZERO;
                a.x = if x_high { S } else { 0.0 };
                a[axis] = if high { S } else { 0.0 };
                let mut b = a;
                b[along] = S;
                rims.push((frame().point(a).as_dvec3(), frame().point(b).as_dvec3()));
            }
        }
    }
    (polygons, slots, rims)
}

// The full leak. Both x faces of a box are gone and each of their
// eight rims is a hinge beside staggered strips, so a ray along x crosses
// the box's solid through faces it lies in. Every rim reads unproven along
// its length, whether the stagger welds or not.
#[test]
fn coverage_proof_reports_every_rim_of_a_box_missing_both_x_faces_beside_strips() {
    for (width, first) in [(0.00098, 0.00049), (0.0004, 0.0004 + 0.8 * delta())] {
        let (polygons, [low, high], rims) = hinged_box(width, first);
        let intact = prove(&polygons);
        assert!(
            intact.unproven.is_empty(),
            "strips {width} m: {:#?}",
            intact.unproven
        );
        let open: Vec<Polygon> = polygons
            .into_iter()
            .enumerate()
            .filter(|&(slot, _)| slot != low && slot != high)
            .map(|(_, p)| p)
            .collect();
        let report = prove(&open);
        for rim in rims {
            let length = (rim.1 - rim.0).length();
            let along = unproven_along(&report, rim);
            assert!(
                along >= length - 2.0 * delta(),
                "strips {width} m: {along} m of a {length} m rim unproven"
            );
        }
    }
}

/// The chained hinge with brush 0's x = 0 side face replaced by `r1`, loops
/// in local coordinates. The dropped face is the one `chained_hinge`
/// removes to make the hole.
fn hinge_with_r1(
    length: f64,
    r1: Vec<Vec<DVec3>>,
    bottom: Vec<Vec<DVec3>>,
) -> (Vec<Polygon>, usize, (DVec3, DVec3)) {
    let (mut polygons, dropped, line) =
        chained_hinge(frame(), length, top_strips(&[0.0, length]), bottom);
    // Order −X, +X, …: the side face is brush 0's slot 1.
    polygons.remove(1);
    let dropped = dropped - 1;
    polygons.extend(
        r1.into_iter()
            .map(|corners| facing(frame(), corners, DVec3::X, 0)),
    );
    (polygons, dropped, line)
}

// The deficit side as a chain. With the dropped face gone, the side face
// is the hinge's one edge left running −y, its corners every 0.4 mm. Uncapped stand-ins let
// each link's neighbours partner the second +y edge.
#[test]
fn coverage_proof_reports_a_hinge_missing_a_face_whose_deficit_side_is_a_chain() {
    let mut r1 = vec![DVec3::new(0.0, 0.0, -1.0), DVec3::new(0.0, 1.0, -1.0)];
    r1.extend((0..=2500).map(|k| DVec3::new(0.0, 1.0 - f64::from(k) / 2500.0, 0.0)));
    let (polygons, dropped, line) = hinge_with_r1(1.0, vec![r1], bottom_strips(&[0.0, 1.0]));
    assert_hinge_hole_found(polygons, dropped, line, 2.0 * delta());
}

/// Windows `[k·pitch + from, k·pitch + to]` along a hinge `length` long.
fn windows(length: f64, pitch: f64, from: f64, to: f64) -> Vec<[f64; 2]> {
    let count = (length / pitch).round() as usize;
    (0..count)
        .map(|k| {
            let base = k as f64 * pitch;
            [base + from, base + to]
        })
        .collect()
}

/// The side face cut into strips at `strips`, with brush 1's bottom only at `bottom`.
fn slotted_hinge(
    length: f64,
    strips: &[[f64; 2]],
    bottom: &[[f64; 2]],
) -> (Vec<Polygon>, usize, (DVec3, DVec3)) {
    let r1 = strips
        .iter()
        .map(|&[y0, y1]| {
            vec![
                DVec3::new(0.0, y0, -1.0),
                DVec3::new(0.0, y1, -1.0),
                DVec3::new(0.0, y1, 0.0),
                DVec3::new(0.0, y0, 0.0),
            ]
        })
        .collect();
    let bottom = bottom
        .iter()
        .flat_map(|&[y0, y1]| bottom_strips(&[y0, y1]))
        .collect();
    hinge_with_r1(length, r1, bottom)
}

// The side face cut into 0.4 mm strips with 0.1 mm slots, brush 1's bottom
// only at the side face's slots, the dropped face gone. Uncapped stand-ins let each slot borrow both
// neighbouring strips. The cap lets a slot lift the −y count only to the
// one edge present beside it, so one of its two +y edges stays unmatched
// on every slot; the strips balance, the bottom's edge against the side face's.
#[test]
fn coverage_proof_reports_a_hinge_whose_deficit_side_is_slotted() {
    const LENGTH: f64 = 0.02;
    let strips = windows(LENGTH, 0.0005, 0.0, 0.0004);
    let slots = windows(LENGTH, 0.0005, 0.0004, 0.0005);
    let (polygons, dropped, line) = slotted_hinge(LENGTH, &strips, &slots);
    let mut open = polygons;
    open.remove(dropped);
    let report = prove(&open);
    let at = |y: f64| line.0 + (line.1 - line.0) * (y / LENGTH);
    for &[y0, y1] in &slots {
        let along = unproven_along(&report, (at(y0), at(y1)));
        assert!(
            along >= (y1 - y0) - 1e-6,
            "{along} m unproven on the slot at {y0} m"
        );
    }
}

// The strips-aligned reading: brush 1's bottom only at the side face's
// strips. Each strip holds two +y edges against the side face's one, and
// the cap lets no −y stand-in in where one −y edge is all that lies within
// τ, so every strip reads unproven. A slot holds the bottom's edge alone, a
// crack under τ between the side face's strips, which their ends close.
#[test]
fn coverage_proof_reports_a_hinge_whose_deficit_side_is_slotted_beside_strips() {
    const LENGTH: f64 = 0.02;
    let strips = windows(LENGTH, 0.0005, 0.0, 0.0004);
    let (polygons, dropped, line) = slotted_hinge(LENGTH, &strips, &strips);
    let mut open = polygons;
    open.remove(dropped);
    let report = prove(&open);
    let at = |y: f64| line.0 + (line.1 - line.0) * (y / LENGTH);
    for &[y0, y1] in &strips {
        let along = unproven_along(&report, (at(y0), at(y1)));
        assert!(
            along >= (y1 - y0) - 1e-6,
            "{along} m unproven on the strip at {y0} m"
        );
    }
}

// The dropped face's hinge edge stops 0.3 mm (over δ, under τ) short of the
// side face's at both ends. The copies sit too far apart to weld; the
// dropped face stands in with its stored
// end on each gap, where the −y count dips below the two present within τ,
// and the gaps are proven as the cracks within τ they are.
#[test]
fn coverage_proof_proves_a_gap_under_tau_at_a_hinge_end_by_a_stand_in() {
    const LENGTH: f64 = 1.0;
    const GAP: f64 = 0.0003;
    assert!(GAP > delta() && GAP < tau());
    let (mut polygons, dropped, _) = chained_hinge(
        frame(),
        LENGTH,
        top_strips(&[0.0, LENGTH]),
        bottom_strips(&[0.0, LENGTH]),
    );
    polygons[dropped] = facing(
        frame(),
        vec![
            DVec3::new(0.0, GAP, 0.0),
            DVec3::new(0.0, GAP, 1.0),
            DVec3::new(0.0, LENGTH - GAP, 1.0),
            DVec3::new(0.0, LENGTH - GAP, 0.0),
        ],
        DVec3::NEG_X,
        1,
    );
    let report = prove(&polygons);
    assert!(report.unproven.is_empty(), "{:#?}", report.unproven);
}

/// The box [0, s]³ without its x = 0 and x = s faces, its other four faces
/// strips along x `width` wide with slots `slot` wide or a little wider, the
/// last strip flush with the far edge. Returns the faces, the eight rims of
/// the missing faces, and the strips per face.
fn fold_comb(s: f64, width: f64, slot: f64) -> (Vec<Polygon>, Vec<(DVec3, DVec3)>, usize) {
    let n = ((s + slot) / (width + slot)).floor() as usize;
    let gap = (s - n as f64 * width) / (n - 1) as f64;
    let mut cuts = Vec::new();
    for k in 0..n {
        let a = k as f64 * (width + gap);
        cuts.push(a);
        cuts.push(if k + 1 == n { s } else { a + width });
    }
    let core = (DVec3::ZERO, DVec3::splat(s));
    let mut polygons = Vec::new();
    for (axis, across) in [(1, 2), (2, 1)] {
        for high in [false, true] {
            // Every other window of the cuts is a strip; the rest are slots.
            let faces = box_face(frame(), core, (axis, high), Some((across, &cuts)), 0);
            polygons.extend(faces.into_iter().step_by(2));
        }
    }
    let mut rims = Vec::new();
    for x in [0.0, s] {
        for fixed in [0.0, s] {
            rims.push((DVec3::new(x, 0.0, fixed), DVec3::new(x, s, fixed)));
            rims.push((DVec3::new(x, fixed, 0.0), DVec3::new(x, fixed, s)));
        }
    }
    let rims = rims
        .into_iter()
        .map(|(a, b)| (frame().point(a).as_dvec3(), frame().point(b).as_dvec3()))
        .collect();
    (polygons, rims, n)
}

// The fold comb. A box missing both x faces, its other faces strips
// 0.5δ wide with slots over δ and under τ. Each strip's two end corners lie
// within δ of each other; welding them would fold every strip away and
// leave nothing to report. The weld refuses a cluster holding two corners
// of one triangle, so every strip end stays on its rim, unmatched.
#[test]
fn coverage_proof_reports_a_fold_comb_missing_both_x_faces() {
    let (width, slot) = (0.5 * delta(), 0.5 * delta() + 100e-6);
    assert!(slot > delta() && slot < 0.5e-3);
    let (polygons, rims, n) = fold_comb(0.02, width, slot);
    let ends = n as f64 * width;
    let report = prove(&polygons);
    for &rim in &rims {
        let along = unproven_near(&report, rim, 1e-6);
        assert!(
            along >= 0.99 * ends,
            "{along} m of the rim's {ends} m of strip ends unproven"
        );
    }
    // Negative control: without the refusal the strips fold away.
    let folded = prove_with(
        &polygons,
        WeldRule {
            radius: delta(),
            keep_triangles_whole: false,
        },
    );
    assert!(
        rims.iter()
            .all(|&rim| unproven_near(&folded, rim, 1e-6) == 0.0),
        "the negative control still reads the rims"
    );
}
