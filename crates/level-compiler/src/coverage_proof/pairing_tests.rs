// Coverage proof tests: which edges may pair — opposed as stored, never two of one face, and stand-ins only within τ measured along the edges in 3D.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use glam::DVec3;

use super::clusters::opposed;
use super::edges::DirectedEdge;
use super::test_fixtures::{
    Frame, axis_skew_hinge, closed_box, delta, facing, lies_along, prove, tau, unproven_along,
};

fn frame() -> Frame {
    Frame::oblique(DVec3::new(1.0, 2.0, 3.0))
}

// A face narrower than τ lies within τ of its own opposite edges. Paired
// with each other, they proved a lone sliver closed, and a solid whose
// faces were all such slivers read proven. Two edges of one face never
// pair, so with nothing around it the sliver's long edges read unproven
// whole.
#[test]
fn coverage_proof_reports_a_lone_face_narrower_than_tau() {
    let width = 0.6 * tau();
    let corner = |x: f64, z: f64| DVec3::new(x, 0.0, z);
    let sliver = facing(
        frame(),
        vec![
            corner(0.0, 0.0),
            corner(width, 0.0),
            corner(width, 1.0),
            corner(0.0, 1.0),
        ],
        DVec3::NEG_Y,
        0,
    );
    let report = prove(&[sliver]);
    assert_eq!(report.edges_proven, 0, "{:#?}", report.unproven);
    for x in [0.0, width] {
        let edge = (
            frame().point(corner(x, 0.0)).as_dvec3(),
            frame().point(corner(x, 1.0)).as_dvec3(),
        );
        let along = unproven_along(&report, edge);
        assert!(
            along >= 1.0 - 1e-6,
            "{along} m of the long edge at x = {x} unproven"
        );
    }
}

// Review attack 4 (axis skew). The hinge's cluster takes E1 as its axis, at
// 89.977° to the hinge, so the whole metre of hinge projects onto 0.4 mm of
// axis. Box B's −X face D is gone below z = 5 cm, and a row of closed
// shells 0.2 mm long every 0.35 mm, each with a crease on the hinge, offers
// a stand-in within τ of every piece there. Each shell piece then needs one
// stand-in, and only a piece whose deficit side holds one edge more than a
// shell piece's can license it: one shell beside D, half a metre up. In
// axis units that shell lay within τ of every piece, so every shell piece
// took a stand-in and the stretch read proven. Along
// the edges in 3D the window holds only neighbouring shells, and every
// shell piece reads unproven. The identity frame keeps the f32 corners on
// the hinge: rounded across it, they would shift breaks by up to 0.15 mm
// along it, through the 4e-4 projection.
#[test]
fn coverage_proof_window_is_measured_along_the_edges_not_the_cluster_axis() {
    const OPEN_TO: f64 = 0.05;
    const SHELL: f64 = 0.2e-3;
    const PERIOD: f64 = 0.35e-3;
    const ACROSS: f64 = 0.003;
    // Above the sliver by more than the link radius, so the shells' own
    // creases across the hinge stay out of its cluster.
    const FIRST: f64 = 0.002;
    let frame = Frame::IDENTITY;
    let (mut polygons, d, (bottom, top)) = axis_skew_hinge(frame);
    let mut brush = 4;
    let mut shell = |z: f64| {
        let faces = closed_box(
            frame,
            DVec3::new(1.0, 1.0 - ACROSS, z),
            DVec3::new(1.0 + ACROSS, 1.0, z + SHELL),
            0.0,
            brush,
        );
        brush += 1;
        faces
    };
    let mut shells = 0u32;
    while FIRST + f64::from(shells) * PERIOD + SHELL <= OPEN_TO - 0.001 {
        polygons.extend(shell(FIRST + f64::from(shells) * PERIOD));
        shells += 1;
    }
    polygons.extend(shell(0.5));
    let intact = prove(&polygons);
    assert!(intact.unproven.is_empty(), "{:#?}", intact.unproven);

    // D only above `OPEN_TO`.
    polygons[d] = facing(
        frame,
        vec![
            DVec3::new(1.0, 1.0, OPEN_TO),
            DVec3::new(1.0, 2.0, OPEN_TO),
            DVec3::new(1.0, 2.0, 1.0),
            DVec3::new(1.0, 1.0, 1.0),
        ],
        DVec3::NEG_X,
        1,
    );
    let report = prove(&polygons);
    let z = |p: DVec3| (p - bottom).dot(top - bottom);
    let on_hinge: Vec<(f64, f64)> = report
        .unproven
        .iter()
        .filter(|span| lies_along(span, bottom, top, 1e-6))
        .map(|span| {
            let (a, b) = (z(span.endpoints[0]), z(span.endpoints[1]));
            (a.min(b), a.max(b))
        })
        .collect();
    for k in 0..shells {
        let lo = FIRST + f64::from(k) * PERIOD;
        let hi = lo + SHELL;
        let unproven: f64 = on_hinge
            .iter()
            .map(|&(a, b)| (b.min(hi) - a.max(lo)).max(0.0))
            .sum();
        assert!(
            unproven >= SHELL - 1e-6,
            "{unproven} m of the shell piece at z = {lo} unproven"
        );
    }
}

// Welding never reverses an edge, but it can turn a short one by nearly
// 90°. Two edges 180 µm long, both running +x, have each end welded 85 µm
// along and 50 µm across: 98.6 µm from its own stored corner, within δ, and
// 107.4 µm from the other, outside it, as the weld allows. One turns about
// 84° one way and the other 84° the other way, so welded they run nearly
// opposite, read opposed, and could pair. Which way an edge runs is read
// from its stored corners.
#[test]
fn coverage_proof_reads_opposition_from_stored_directions_not_welded() {
    let edge = |from: DVec3, to: DVec3, weld_from: DVec3, weld_to: DVec3, face: u32| DirectedEdge {
        from,
        to,
        weld_from,
        weld_to,
        face,
        triangle: 0,
        brush: None,
    };
    let (length, along, across) = (1.8 * delta(), 0.85 * delta(), 0.5 * delta());
    // Apart by 3δ, so no corner of one edge welds with a corner of the other.
    let at = |x: f64, y: f64, z: f64| DVec3::new(x, y, 3.0 * delta() * z);
    // Stored +x on the line at `z`, its ends welded across it in opposite
    // senses, so it turns toward −`sense` y.
    let turned = |z: f64, sense: f64, face: u32| {
        edge(
            at(0.0, 0.0, z),
            at(length, 0.0, z),
            at(along, sense * across, z),
            at(length - along, -sense * across, z),
            face,
        )
    };
    let (up, down) = (turned(0.0, -1.0, 0), turned(1.0, 1.0, 1));
    for e in [&up, &down] {
        for (welded, own, other) in [(e.weld_from, e.from, e.to), (e.weld_to, e.to, e.from)] {
            assert!(
                welded.distance(own) <= delta() && welded.distance(other) > delta(),
                "each welded end lies within δ of its own corner only"
            );
        }
        let turn = e.direction().dot(e.stored_direction());
        assert!(
            (0.0..0.15).contains(&turn),
            "the weld turns the edge {}°: nearly 90°, never past it",
            turn.acos().to_degrees()
        );
    }
    assert!(
        up.direction().dot(down.direction()) <= -0.5,
        "welded, the same-way edges read opposed"
    );
    assert!(!opposed(&up, &down));

    // Stored −x, unwelded: opposed.
    let against = edge(
        at(length, 0.0, 1.0),
        at(0.0, 0.0, 1.0),
        at(length, 0.0, 1.0),
        at(0.0, 0.0, 1.0),
        2,
    );
    assert!(opposed(&up, &against));
}
