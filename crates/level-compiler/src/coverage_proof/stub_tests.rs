// Coverage proof tests: the end-stub rule's counterexamples, and overlaps at corners the surface does and does not close round.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use glam::DVec3;

use super::CoverageReport;
use super::test_fixtures::{
    Frame, Polygon, closed_box, hinge, lies_along, prove, tau, tetrahedron,
};

/// Total length of the unproven spans lying along the segment `a`–`b`.
fn unproven_along(report: &CoverageReport, (a, b): (DVec3, DVec3)) -> f64 {
    report
        .unproven
        .iter()
        .filter(|span| lies_along(span, a, b, 1e-6))
        .map(|span| span.length)
        .sum()
}

/// The `hinge` fixture plus a 2 m plank whose edge E1 runs into the hinge's
/// bottom end at 89.977° to it, and a sub-millimetre sliver whose 0.7 mm
/// edge runs opposite both lines near that end, linking them into one
/// cluster with E1 as its axis. Every brush is closed. Returns the faces,
/// box B's −X slot (a hinge face) and the hinge's ends.
fn axis_skew_hinge(frame: Frame) -> (Vec<Polygon>, usize, (DVec3, DVec3)) {
    let (mut polygons, slots, line) = hinge(frame);
    let corner = DVec3::new(1.0, 1.0, 0.0);
    let far = DVec3::new(1.0 + 2f64.sqrt(), 1.0 - 2f64.sqrt(), 0.0008);
    let middle = (far + corner) * 0.5;
    let outward = DVec3::new(1.0, 1.0, 0.0).normalize();
    polygons.extend(tetrahedron(
        frame,
        [far, corner, middle + outward * 0.1, middle + DVec3::Z * 0.1],
        2,
    ));

    let (u, v) = ((corner - far).normalize(), DVec3::Z);
    let d = -(u + v).normalize();
    let s0 = corner + DVec3::Z * 0.0006;
    let s1 = s0 + d * 0.0007;
    let aside = DVec3::new(1.0, -1.0, 0.0).normalize();
    let lift = d.cross(aside).normalize();
    polygons.extend(tetrahedron(
        frame,
        [s0, s1, s0 + aside * 0.0005, (s0 + s1) * 0.5 + lift * 0.0004],
        3,
    ));
    (polygons, slots[2], line)
}

// Regression: the end-stub rule measured a stub along its cluster's axis.
// One 0.7 mm edge merged a hinge into a cluster whose axis lay across it,
// every hinge edge projected under τ and read as a stub, and the hinge
// missing a face read proven along its whole metre.
#[test]
fn coverage_proof_reports_a_hinge_missing_a_face_whose_cluster_axis_lies_across_it() {
    let frame = Frame::oblique(DVec3::new(1.0, 2.0, 3.0));
    let (polygons, dropped, line) = axis_skew_hinge(frame);

    let intact = prove(&polygons);
    assert!(
        intact.unproven.is_empty(),
        "closed brushes, skewed cluster: {:#?}",
        intact.unproven
    );

    let mut open = polygons;
    open.remove(dropped);
    let report = prove(&open);
    // The sliver touches the hinge's first half millimetre, within τ of it.
    let along = unproven_along(&report, line);
    assert!(
        along >= 1.0 - 2.0 * tau(),
        "{along} m of the 1 m hinge unproven: {:#?}",
        report.unproven
    );
}

/// Two brushes meeting on the hinge x = z = 0, 0 ≤ y ≤ `length` (local):
/// brush 0 = [−1, 0] × [0, length] × [−1, 0] and brush 1 = [0, 1] ×
/// [0, length] × [0, 1]. Brush 0's top (z = 0) is replaced by `top` and
/// brush 1's bottom by `bottom`, each a list of local loops. Returns the
/// faces, brush 1's −X slot (a hinge face) and the hinge's ends.
fn chained_hinge(
    frame: Frame,
    length: f64,
    top: Vec<Vec<DVec3>>,
    bottom: Vec<Vec<DVec3>>,
) -> (Vec<Polygon>, usize, (DVec3, DVec3)) {
    let face = |brush: u32| {
        move |corners: Vec<DVec3>| Polygon {
            corners: corners.iter().map(|&p| frame.point(p)).collect(),
            brush: Some(brush),
        }
    };
    let mut polygons = closed_box(
        frame,
        DVec3::new(-1.0, 0.0, -1.0),
        DVec3::new(0.0, length, 0.0),
        0.0,
        0,
    );
    // Order −X, +X, −Y, +Y, −Z, +Z: brush 0's top is slot 5.
    polygons.remove(5);
    polygons.extend(top.into_iter().map(face(0)));
    let mut second = closed_box(frame, DVec3::ZERO, DVec3::new(1.0, length, 1.0), 0.0, 1);
    second.remove(4);
    let dropped = polygons.len();
    polygons.extend(second);
    polygons.extend(bottom.into_iter().map(face(1)));
    let line = (
        frame.point(DVec3::ZERO).as_dvec3(),
        frame.point(DVec3::Y * length).as_dvec3(),
    );
    (polygons, dropped, line)
}

/// Hinge stations: 0, then every `step` from `first`, then `length`.
fn stations(length: f64, first: f64, step: f64) -> Vec<f64> {
    let mut ys = vec![0.0];
    let mut k = 0.0;
    while first + step * k < length - 1e-9 {
        ys.push(first + step * k);
        k += 1.0;
    }
    ys.push(length);
    ys.dedup();
    ys
}

/// Brush 0's top as strips between consecutive `ys`.
fn top_strips(ys: &[f64]) -> Vec<Vec<DVec3>> {
    ys.windows(2)
        .map(|w| {
            vec![
                DVec3::new(-1.0, w[0], 0.0),
                DVec3::new(0.0, w[0], 0.0),
                DVec3::new(0.0, w[1], 0.0),
                DVec3::new(-1.0, w[1], 0.0),
            ]
        })
        .collect()
}

/// Brush 1's bottom as strips between consecutive `ys`.
fn bottom_strips(ys: &[f64]) -> Vec<Vec<DVec3>> {
    ys.windows(2)
        .map(|w| {
            vec![
                DVec3::new(0.0, w[0], 0.0),
                DVec3::new(0.0, w[1], 0.0),
                DVec3::new(1.0, w[1], 0.0),
                DVec3::new(1.0, w[0], 0.0),
            ]
        })
        .collect()
}

/// Assert `polygons` prove closed, and that without the face at `dropped`
/// the whole hinge `line` reads unproven.
fn assert_hinge_hole_found(polygons: Vec<Polygon>, dropped: usize, line: (DVec3, DVec3)) {
    let length = (line.1 - line.0).length();
    let intact = prove(&polygons);
    assert!(
        intact.unproven.is_empty(),
        "closed brushes: {:#?}",
        intact.unproven
    );
    let mut open = polygons;
    open.remove(dropped);
    let report = prove(&open);
    let along = unproven_along(&report, line);
    assert!(
        along >= length - 1e-6,
        "{along} m of the {length} m hinge unproven ({} spans)",
        report.unproven.len()
    );
}

// Regression: under the end-stub rule a stub needed only some partner, not
// one of its own. A face edge broken into 0.4 mm links, every one a stub,
// shared the one opposite edge left on a hinge missing a face.
#[test]
fn coverage_proof_reports_a_hinge_missing_a_face_beside_a_chain_of_sub_tau_edges() {
    let frame = Frame::oblique(DVec3::new(1.0, 2.0, 3.0));
    // Brush 0's top, one loop with a collinear corner every 0.4 mm on the
    // hinge, fanned from a corner off it.
    let mut top = vec![DVec3::new(-1.0, 0.0, 0.0)];
    top.extend((0..=2500).map(|k| DVec3::new(0.0, f64::from(k) / 2500.0, 0.0)));
    top.push(DVec3::new(-1.0, 1.0, 0.0));
    let bottom = bottom_strips(&[0.0, 1.0]);
    let (polygons, dropped, line) = chained_hinge(frame, 1.0, vec![top], bottom);
    assert_hinge_hole_found(polygons, dropped, line);
}

// Regression: both same-way faces on a hinge tiled into 0.4 mm strips,
// staggered by 0.2 mm, so every piece holds two same-way edges whose ends
// overlap by under τ. The faces turn off the hinge the same way at those
// ends, so no seam explains the overlap. A 2 cm hinge: strip seams 0.4 mm
// apart link into one cluster, so its cost grows with the strip count
// squared.
#[test]
fn coverage_proof_reports_a_hinge_missing_a_face_beside_staggered_sub_tau_strips() {
    const LENGTH: f64 = 0.02;
    let frame = Frame::oblique(DVec3::new(1.0, 2.0, 3.0));
    let top = top_strips(&stations(LENGTH, 0.0004, 0.0004));
    let bottom = bottom_strips(&stations(LENGTH, 0.0002, 0.0004));
    let (polygons, dropped, line) = chained_hinge(frame, LENGTH, top, bottom);
    assert_hinge_hole_found(polygons, dropped, line);
}

/// `corners` (local) in `frame` as one face of `brush`, wound
/// counter-clockwise about `outward`.
fn facing(frame: Frame, mut corners: Vec<DVec3>, outward: DVec3, brush: u32) -> Polygon {
    let normal = (corners[1] - corners[0]).cross(corners[2] - corners[1]);
    if normal.dot(outward) < 0.0 {
        corners.reverse();
    }
    Polygon {
        corners: corners.iter().map(|&p| frame.point(p)).collect(),
        brush: Some(brush),
    }
}

/// A wall block [−1, 0] × [−1, 1] × [−1, 1] with a beam against its +X face
/// above the line x = y = 0 (local): the beam's flat bottom (y = 0) and
/// sloped face meet the wall at P = origin. The wall face splits along that
/// line, as a cell boundary splits it. The wall's copy of P and the
/// bottom's land `apart` metres apart along the line, overlapping, while
/// the sloped face keeps P; the sloped face closes the corner between them.
/// Returns the faces, the sloped face's slot, and P in world space.
fn beam_corner(frame: Frame, apart: f64) -> (Vec<Polygon>, usize, DVec3) {
    let (p, a, c, e) = (
        DVec3::ZERO,
        DVec3::new(0.0, 0.0, -1.0),
        DVec3::new(0.0, 1.0, -1.0),
        DVec3::new(1.0, 0.0, -1.0),
    );
    let (wall_p, bottom_p) = (DVec3::Z * -0.5 * apart, DVec3::Z * 0.5 * apart);
    let mut polygons = closed_box(frame, DVec3::splat(-1.0), DVec3::new(0.0, 1.0, 1.0), 0.0, 0);
    // Order −X, +X, …: the wall's +X face is slot 1.
    polygons.remove(1);
    polygons.push(facing(
        frame,
        vec![
            DVec3::new(0.0, -1.0, -1.0),
            DVec3::new(0.0, 0.0, -1.0),
            DVec3::new(0.0, 0.0, 1.0),
            DVec3::new(0.0, -1.0, 1.0),
        ],
        DVec3::X,
        0,
    ));
    polygons.push(facing(
        frame,
        vec![wall_p, DVec3::Z, DVec3::new(0.0, 1.0, 1.0), c],
        DVec3::X,
        0,
    ));
    polygons.push(facing(frame, vec![bottom_p, a, e], DVec3::NEG_Y, 1));
    polygons.push(facing(frame, vec![a, c, e], DVec3::NEG_Z, 1));
    let sloped = polygons.len();
    polygons.push(facing(frame, vec![p, c, e], DVec3::ONE, 1));
    (polygons, sloped, frame.point(p).as_dvec3())
}

// The movement-feel corners: three faces meet where a wall meets a beam's
// flat bottom and sloped face, and the wall's and the bottom's copies of
// the corner land micrometres apart along the wall line. The edges where
// the two turn off the line meet through the sloped face, not directly.
#[test]
fn coverage_proof_proves_an_overlap_at_a_corner_the_surface_closes_round() {
    let frame = Frame::oblique(DVec3::new(-3.0, 4.0, 1.0));
    let (polygons, _, _) = beam_corner(frame, 10e-6);
    let report = prove(&polygons);
    assert!(
        report.unproven.is_empty(),
        "a closed corner: {:#?}",
        report.unproven
    );
}

// Without the face closing the corner, the overlap there has no fan to
// explain it.
#[test]
fn coverage_proof_reports_an_overlap_at_a_corner_missing_its_closing_face() {
    let frame = Frame::oblique(DVec3::new(-3.0, 4.0, 1.0));
    let (mut polygons, sloped, corner) = beam_corner(frame, 10e-6);
    polygons.remove(sloped);
    let report = prove(&polygons);
    assert!(
        report.unproven.iter().any(|span| {
            span.length < 1e-4 && span.endpoints.iter().all(|&p| (p - corner).length() < 1e-4)
        }),
        "no unproven overlap at the corner: {:#?}",
        report.unproven
    );
}
