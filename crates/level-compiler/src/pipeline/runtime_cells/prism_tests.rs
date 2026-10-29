// Shadow-prism rows: which occluding faces sweep a prism, which of their
// vertical end edges are free and offer a splitter, the length bound
// deciding eligibility both ways, the overlap tolerance, and order
// independence.
// See: context/plans/in-progress/occluder-first-partition/index.md §Decisions

use glam::DVec3;

use super::{FirstPass, classify};
use crate::map_data::BrushVolume;
use crate::partition::occluder_classify::test_fixtures::{
    box_brush, chain_walls, cube_in_room, hall, retaining_wall, street_deck,
};
use crate::partition::occluder_classify::{OCCLUDER_FLOOR_M, OccluderVerdict};
use crate::partition::occluder_cluster::test_fixtures::strip_facade;
use crate::partition::region_polytope::RegionPolytope;
use crate::partition::{
    Aabb, FREE_END_MIN_OPEN_PROBES, FREE_END_PROBE_DEPTHS_M, FREE_END_PROBE_FRACTIONS,
    FREE_END_PROBE_OFFSET_M, SHADOW_PRISM_LENGTH_FACTOR, ShadowPrism, shadow_prisms,
};

/// Tolerance on prism measures, which come from the first pass's segment
/// queries and own-frame boxes.
const MEASURE_EPS: f64 = 1e-6;

/// Step either side of the length bound in the rows that decide it both ways.
const BOUND_STEP_M: f64 = 0.25;

/// Index of the first brush a fixture adds after `hall`'s six slabs.
const AFTER_HALL: usize = 6;

/// The shadow prisms of `brushes`, classified and measured against their own
/// first pass, as the production partition builds them.
fn prisms_of(brushes: &[BrushVolume]) -> Vec<ShadowPrism> {
    let classification = classify(brushes).expect("classifies");
    let first_pass = FirstPass::build(brushes)
        .expect("first pass builds")
        .expect("brushes");
    shadow_prisms(brushes, &classification, &first_pass.space())
}

/// The prisms swept from the face whose outward normal is `normal` on the
/// plane `normal · p = distance`.
fn prisms_on_face(prisms: &[ShadowPrism], normal: DVec3, distance: f64) -> Vec<&ShadowPrism> {
    prisms
        .iter()
        .filter(|prism| {
            (prism.face_normal() - normal).length() < MEASURE_EPS
                && (prism.face_distance() - distance).abs() < MEASURE_EPS
        })
        .collect()
}

/// The one prism swept from that face.
fn prism_on_face(prisms: &[ShadowPrism], normal: DVec3, distance: f64) -> &ShadowPrism {
    let on_face = prisms_on_face(prisms, normal, distance);
    assert_eq!(
        on_face.len(),
        1,
        "one prism on the face {normal:?} · p = {distance}: {prisms:#?}"
    );
    on_face[0]
}

fn has_splitter(prism: &ShadowPrism, (normal, distance): (DVec3, f64)) -> bool {
    prism
        .splitters
        .iter()
        .any(|&(n, d)| (n - normal).length() < MEASURE_EPS && (d - distance).abs() < MEASURE_EPS)
}

/// Asserts `prism`'s splitters are exactly `expected`, in any order.
fn assert_splitters(prism: &ShadowPrism, expected: &[(DVec3, f64)], what: &str) {
    assert_eq!(
        prism.splitters.len(),
        expected.len(),
        "{what}: splitters {:?}",
        prism.splitters
    );
    for &splitter in expected {
        assert!(
            has_splitter(prism, splitter),
            "{what}: lacks splitter {splitter:?}: {:?}",
            prism.splitters
        );
    }
}

/// A wall 8 m long, 4 m tall and 0.3 m thick across a hall 14 m wide and
/// 6 m tall, 5 m from its -z end wall, with `depth_behind` of open hall
/// behind it. It fills over half the hall's span, so it occludes; its back
/// prism's length bound is `SHADOW_PRISM_LENGTH_FACTOR` × 8 m.
fn bounded_wall_hall(depth_behind: f64) -> Vec<BrushVolume> {
    let mut brushes = hall(DVec3::new(14.0, 6.0, WALL_BACK_Z + depth_behind));
    brushes.push(box_brush(
        DVec3::new(3.0, 0.0, 5.0),
        DVec3::new(3.0 + BOUNDED_WALL_LENGTH_M, 4.0, WALL_BACK_Z),
    ));
    brushes
}

const BOUNDED_WALL_LENGTH_M: f64 = 8.0;
const WALL_BACK_Z: f64 = 5.3;

fn bounded_wall_bound() -> f64 {
    SHADOW_PRISM_LENGTH_FACTOR * BOUNDED_WALL_LENGTH_M
}

#[test]
fn shadow_prism_of_a_retaining_wall_offers_only_its_free_end_planes_from_each_broad_face() {
    // The wall spans x 13..39, y 0..6.5, z 30..30.81 in a hall 52 m wide
    // and 60 m long: 29.19 m of open hall behind it and 30 m in front, both
    // under twice its 26 m length, so neither prism is truncated. Both ends
    // stand 13 m from the hall walls, so both are free; its top and bottom
    // edges add no splitter. The hall's slabs occlude, but each fronts the
    // void outside, where no eye stands, so they sweep none: a room's own
    // walls never cut the room.
    let fixture = retaining_wall();
    let prisms = prisms_of(&fixture.brushes);
    assert_eq!(prisms.len(), 2, "only the wall sweeps prisms: {prisms:#?}");
    for (normal, face, depth) in [(DVec3::Z, 30.81, 60.0 - 30.81), (DVec3::NEG_Z, -30.0, 30.0)] {
        let prism = prism_on_face(&prisms, normal, face);
        assert!(
            (prism.depth - depth).abs() < MEASURE_EPS,
            "the prism ends at the hall wall: {} against {depth}",
            prism.depth
        );
        assert!(!prism.truncated);
        assert_splitters(
            prism,
            &[(DVec3::X, 13.0), (DVec3::NEG_X, -39.0)],
            "the two vertical end planes",
        );
    }
}

#[test]
fn shadow_prism_length_bound_decides_eligibility_both_ways_and_adds_no_far_plane() {
    let bound = bounded_wall_bound();
    let back_prism = |depth_behind: f64| {
        let prisms = prisms_of(&bounded_wall_hall(depth_behind));
        prism_on_face(&prisms, DVec3::Z, WALL_BACK_Z).clone()
    };
    let ends = [
        (DVec3::X, 3.0),
        (DVec3::NEG_X, -(3.0 + BOUNDED_WALL_LENGTH_M)),
    ];

    // Just inside the bound: the open space ends the prism.
    let inside = back_prism(bound - BOUND_STEP_M);
    assert!(!inside.truncated, "the open space ends the prism first");
    assert!((inside.depth - (bound - BOUND_STEP_M)).abs() < MEASURE_EPS);
    assert_splitters(&inside, &ends, "inside the bound");

    // Just outside it: the bound ends the prism, and still only the end
    // planes split.
    let outside = back_prism(bound + BOUND_STEP_M);
    assert!(outside.truncated, "the length bound ends the prism first");
    assert!((outside.depth - bound).abs() < MEASURE_EPS);
    assert_splitters(&outside, &ends, "past the bound, no far plane");

    // Behind the wall, a slab of region reaching just into the prism's far
    // end is offered its end planes; one starting just past it is not.
    let far = WALL_BACK_Z + bound;
    let slab = |z0: f64, z1: f64| {
        RegionPolytope::from_aabb(&Aabb {
            min: DVec3::new(0.0, 0.0, z0),
            max: DVec3::new(14.0, 6.0, z1),
        })
    };
    let half = BOUND_STEP_M * 0.5;
    assert!(
        outside.overlaps(&slab(far - half, far + half)),
        "a region reaching into the prism is eligible"
    );
    assert!(
        !outside.overlaps(&slab(far + half, far + BOUND_STEP_M)),
        "a region past the length bound is not"
    );
}

#[test]
fn shadow_prism_needs_open_space_at_least_the_floor_deep_in_front_of_both_broad_faces() {
    // A wall 24 m long, so it occludes on length alone, stands `gap` off
    // the hall's -z wall: its back face fronts only the gap. Under the
    // floor, the gap sweeps no prism, and no eye stands in it for the hall
    // side's prism to hide anything from.
    let wall_prisms = |gap: f64| {
        let mut brushes = hall(DVec3::new(40.0, 6.0, 20.0));
        brushes.push(box_brush(
            DVec3::new(8.0, 0.0, gap),
            DVec3::new(32.0, 4.0, gap + 0.3),
        ));
        let wall = brushes.len() - 1;
        let classification = classify(&brushes).expect("classifies");
        assert_eq!(classification.verdicts[wall], OccluderVerdict::Occludes);
        let prisms = prisms_of(&brushes);
        (
            prisms_on_face(&prisms, DVec3::NEG_Z, -gap).len(),
            prisms_on_face(&prisms, DVec3::Z, gap + 0.3).len(),
        )
    };
    assert_eq!(
        wall_prisms(OCCLUDER_FLOOR_M - 0.2),
        (0, 0),
        "with a gap under the floor behind it, neither face sweeps a prism"
    );
    assert_eq!(
        wall_prisms(OCCLUDER_FLOOR_M + 0.2),
        (1, 1),
        "with a gap over the floor, both faces sweep one"
    );
}

#[test]
fn shadow_prism_wall_end_meeting_another_wall_adds_no_splitter_while_its_free_end_does() {
    // The chain (pin R28): the second wall, z 10.2..10.5, runs from the
    // 21 m wall's side at x = first_x + 0.3 to a free end at x = 37.5; the
    // third, x mid ± 0.15, runs from the second's +z face to a free end at
    // z = 22.5. Each wall's prisms offer only the plane of its free end.
    let fixture = chain_walls();
    let prisms = prisms_of(&fixture.brushes);
    let (t, first_x) = (0.3, 40.0 - 2.5 - 12.0 - 0.3);
    let abutting_x = first_x + t;
    for (normal, face) in [(DVec3::NEG_Z, -10.2), (DVec3::Z, 10.5)] {
        let prism = prism_on_face(&prisms, normal, face);
        assert_splitters(
            prism,
            &[(DVec3::NEG_X, -37.5)],
            "second wall: its free end only",
        );
        assert!(!has_splitter(prism, (DVec3::X, abutting_x)));
    }
    let mid_x = (abutting_x + 37.5) * 0.5;
    for (normal, face) in [
        (DVec3::NEG_X, -(mid_x - t * 0.5)),
        (DVec3::X, mid_x + t * 0.5),
    ] {
        let prism = prism_on_face(&prisms, normal, face);
        assert_splitters(
            prism,
            &[(DVec3::NEG_Z, -22.5)],
            "third wall: its free end only",
        );
        assert!(!has_splitter(prism, (DVec3::Z, 10.5)));
    }
}

/// A wall 24 m long and 4 m tall, x 8..32, z 8..8.3, in a hall 40 × 8 × 20 m,
/// with blockers just past its +x end behind its +z face covering
/// `blocked` of that end's free-end probes: whole rows of probes along the
/// end from the floor up, then the rest at the shallowest depths of the
/// next row up. Each blocker stands clear of the wall and of the others.
fn partly_blocked_end(blocked: usize) -> Vec<BrushVolume> {
    const END_X: f64 = 32.0;
    const BACK_Z: f64 = 8.3;
    const HEIGHT: f64 = 4.0;
    let mut brushes = hall(DVec3::new(40.0, 8.0, 20.0));
    brushes.push(box_brush(
        DVec3::new(8.0, 0.0, 8.0),
        DVec3::new(END_X, HEIGHT, BACK_Z),
    ));
    // Probe heights from the floor, and the bands between them.
    let rows: Vec<f64> = FREE_END_PROBE_FRACTIONS
        .iter()
        .map(|&fraction| fraction * HEIGHT)
        .collect();
    let depths = FREE_END_PROBE_DEPTHS_M;
    let (whole_rows, rest) = (blocked / depths.len(), blocked % depths.len());
    let between = |a: f64, b: f64| (a + b) * 0.5;
    // Past the end, clear of the wall, and over the probes' line.
    let (x0, x1) = (
        END_X + FREE_END_PROBE_OFFSET_M * 0.5,
        END_X + FREE_END_PROBE_OFFSET_M * 4.0,
    );
    let gap = 0.05;
    let top_of_whole = between(rows[whole_rows - 1], rows[whole_rows]);
    brushes.push(box_brush(
        DVec3::new(x0, 0.0, BACK_Z),
        DVec3::new(x1, top_of_whole, BACK_Z + depths[depths.len() - 1] + 0.5),
    ));
    if rest > 0 {
        brushes.push(box_brush(
            DVec3::new(x0, top_of_whole + gap, BACK_Z),
            DVec3::new(
                x1,
                between(rows[whole_rows], rows[whole_rows + 1]),
                BACK_Z + between(depths[rest - 1], depths[rest]),
            ),
        ));
    }
    brushes
}

#[test]
fn shadow_prism_end_is_free_when_most_of_its_probes_beyond_are_open() {
    let total = FREE_END_PROBE_FRACTIONS.len() * FREE_END_PROBE_DEPTHS_M.len();
    let free_blocked = total - FREE_END_MIN_OPEN_PROBES;
    assert!(
        free_blocked >= FREE_END_PROBE_DEPTHS_M.len(),
        "precondition: the fixture blocks at least one whole row"
    );
    let back_prism_ends = |blocked: usize| {
        let brushes = partly_blocked_end(blocked);
        let classification = classify(&brushes).expect("classifies");
        assert_eq!(
            classification.verdicts[AFTER_HALL],
            OccluderVerdict::Occludes,
            "precondition: the wall occludes"
        );
        let prisms = prisms_of(&brushes);
        let front = prism_on_face(&prisms, DVec3::NEG_Z, -8.0);
        assert_splitters(
            front,
            &[(DVec3::X, 8.0), (DVec3::NEG_X, -32.0)],
            "the front face's ends see no blocker",
        );
        let back = prism_on_face(&prisms, DVec3::Z, 8.3);
        assert!(has_splitter(back, (DVec3::X, 8.0)), "the other end is free");
        has_splitter(back, (DVec3::NEG_X, -32.0))
    };
    assert!(
        back_prism_ends(free_blocked),
        "{FREE_END_MIN_OPEN_PROBES} of {total} probes open: the end is free"
    );
    assert!(
        !back_prism_ends(free_blocked + 1),
        "{} of {total} probes open: the end is not free",
        FREE_END_MIN_OPEN_PROBES - 1
    );
}

#[test]
fn shadow_prisms_sweep_no_slab_face_and_no_cluster_section() {
    // The street deck occludes, but as a slab: its broad axes are both
    // horizontal, so it sweeps nothing.
    let deck = street_deck();
    let classification = classify(&deck.brushes).expect("classifies");
    assert_eq!(
        classification.verdicts[AFTER_HALL],
        OccluderVerdict::Occludes
    );
    let prisms = prisms_of(&deck.brushes);
    assert!(prisms.is_empty(), "the deck sweeps none: {prisms:#?}");

    // The 10 m cube offers every face; only its four walls sweep, each with
    // both vertical ends free, and its top sweeps none.
    let prisms = prisms_of(&cube_in_room().brushes);
    assert_eq!(prisms.len(), 4, "the cube's four walls: {prisms:#?}");
    for prism in &prisms {
        assert!(prism.face_normal().y.abs() < MEASURE_EPS, "a wall face");
        assert_eq!(prism.splitters.len(), 2, "both ends free");
        for &(normal, _) in &prism.splitters {
            assert!(normal.y.abs() < MEASURE_EPS, "only vertical end planes");
        }
    }

    // The strip facade occludes as a section, its members exempt alone: no
    // prism of either.
    let cluster = strip_facade();
    let brushes = &cluster.fixture.brushes;
    let classification = classify(brushes).expect("classifies");
    assert!(
        classification
            .clusters
            .iter()
            .flat_map(|cluster| &cluster.sections)
            .any(|section| section.occludes),
        "precondition: the facade occludes as a section"
    );
    let prisms = prisms_of(brushes);
    assert!(prisms.is_empty(), "the facade sweeps none: {prisms:#?}");
}

/// A wall 24 m long, 4 m tall and 0.3 m thick, x 8..32, z 3..3.3, in a hall
/// 40 × 6 × 22 m. Its -z face stands 1 m, under the floor, off a filler
/// block across the hall's -z end, except behind x 8..`niche_end`, where a
/// niche in the filler deepens the gap to 3 m. The face's lattice samples
/// stand at x 12, 20 and 28, so a niche to x 16 deepens one column of three
/// and a niche to x 24 two.
fn wall_before_niche(niche_end: f64) -> Vec<BrushVolume> {
    let mut brushes = hall(DVec3::new(40.0, 6.0, 22.0));
    brushes.push(box_brush(
        DVec3::new(8.0, 0.0, 3.0),
        DVec3::new(32.0, 4.0, 3.3),
    ));
    brushes.push(box_brush(DVec3::ZERO, DVec3::new(8.0, 6.0, 2.0)));
    brushes.push(box_brush(
        DVec3::new(niche_end, 0.0, 0.0),
        DVec3::new(40.0, 6.0, 2.0),
    ));
    brushes
}

#[test]
fn shadow_prism_needs_most_of_each_broad_face_open_samples_floor_deep() {
    // The occluder rule's face test: a face counts when most of its open
    // samples front open space at least the floor deep, not when any one
    // does. With one column of the -z face's samples deep, the face does not
    // count, so neither face sweeps a prism; with two, both do.
    let wall_prisms = |niche_end: f64| {
        let brushes = wall_before_niche(niche_end);
        let classification = classify(&brushes).expect("classifies");
        assert_eq!(
            classification.verdicts[AFTER_HALL],
            OccluderVerdict::Occludes,
            "precondition: the wall occludes from its +z face"
        );
        let prisms = prisms_of(&brushes);
        (
            prisms_on_face(&prisms, DVec3::NEG_Z, -3.0).len(),
            prisms_on_face(&prisms, DVec3::Z, 3.3).len(),
        )
    };
    assert_eq!(
        wall_prisms(16.0),
        (0, 0),
        "three of nine samples floor-deep: the -z face does not count"
    );
    assert_eq!(
        wall_prisms(24.0),
        (1, 1),
        "six of nine samples floor-deep: the -z face counts"
    );
}

/// A hall 48 × 8 × 20 m crossed by walls 4 m tall, one per
/// `(x0, x1, z0, z1)`.
fn walls_in_hall(walls: &[(f64, f64, f64, f64)]) -> Vec<BrushVolume> {
    let mut brushes = hall(DVec3::new(48.0, 8.0, 20.0));
    for &(x0, x1, z0, z1) in walls {
        brushes.push(box_brush(DVec3::new(x0, 0.0, z0), DVec3::new(x1, 4.0, z1)));
    }
    brushes
}

/// Two walls 20 m long and 0.3 m thick, meeting end to end at x = 24.
fn in_line_walls() -> Vec<BrushVolume> {
    walls_in_hall(&[(4.0, 24.0, 8.0, 8.3), (24.0, 44.0, 8.0, 8.3)])
}

/// A wall 24 m long built as two layers 0.15 m thick, back to back.
fn two_layer_wall() -> Vec<BrushVolume> {
    walls_in_hall(&[(8.0, 32.0, 8.0, 8.15), (8.0, 32.0, 8.15, 8.3)])
}

/// True when `splitter` lies on the plane `normal · p = distance`, facing
/// either way.
fn on_plane(splitter: (DVec3, f64), (normal, distance): (DVec3, f64)) -> bool {
    let same = |(n, d): (DVec3, f64)| {
        (splitter.0 - n).length() < MEASURE_EPS && (splitter.1 - d).abs() < MEASURE_EPS
    };
    same((normal, distance)) || same((-normal, -distance))
}

// Regression: the free-end probes sat only in front of the face past the
// end, never inside the wall's own thickness, so a joint between two walls
// in line read as two free ends and cut the hall on the joint's plane.
#[test]
fn shadow_prism_joint_between_walls_in_line_adds_no_splitter() {
    let brushes = in_line_walls();
    let classification = classify(&brushes).expect("classifies");
    for wall in [AFTER_HALL, AFTER_HALL + 1] {
        assert_eq!(
            classification.verdicts[wall],
            OccluderVerdict::Occludes,
            "precondition: wall {wall} occludes"
        );
    }
    let prisms = prisms_of(&brushes);
    for (normal, face) in [(DVec3::NEG_Z, -8.0), (DVec3::Z, 8.3)] {
        let on_face = prisms_on_face(&prisms, normal, face);
        assert_eq!(on_face.len(), 2, "each wall sweeps one: {prisms:#?}");
        let mut splitters: Vec<(DVec3, f64)> = on_face
            .iter()
            .flat_map(|prism| prism.splitters.iter().copied())
            .collect();
        assert!(
            !splitters
                .iter()
                .any(|&splitter| on_plane(splitter, (DVec3::X, 24.0))),
            "the joint at x = 24 splits nothing: {splitters:?}"
        );
        splitters.sort_by(|a, b| a.1.total_cmp(&b.1));
        assert_eq!(splitters.len(), 2, "the two outer ends: {splitters:?}");
        assert!(on_plane(splitters[0], (DVec3::X, 44.0)), "{splitters:?}");
        assert!(on_plane(splitters[1], (DVec3::X, 4.0)), "{splitters:?}");
    }
}

// Regression: a perpendicular wall thinner than the free-end probes' offset
// left every probe past the end open, so the end read as free.
#[test]
fn shadow_prism_end_meeting_a_thin_perpendicular_wall_adds_no_splitter() {
    // The cross wall, 0.05 m thick, stands across the end at x = 24, from
    // 6 m in front of the wall to 5.7 m behind it.
    let brushes = walls_in_hall(&[(4.0, 24.0, 8.0, 8.3), (24.0, 24.05, 2.0, 14.0)]);
    let classification = classify(&brushes).expect("classifies");
    assert_eq!(
        classification.verdicts[AFTER_HALL],
        OccluderVerdict::Occludes,
        "precondition: the wall occludes"
    );
    let prisms = prisms_of(&brushes);
    for (normal, face) in [(DVec3::NEG_Z, -8.0), (DVec3::Z, 8.3)] {
        assert_splitters(
            prism_on_face(&prisms, normal, face),
            &[(DVec3::X, 4.0)],
            "the end meeting the cross wall splits nothing",
        );
    }
}

// Regression: each layer of a wall built in layers found its inner face
// fronting the other layer, so neither swept a prism and the space behind
// the wall stayed in cells that eyes past its ends saw.
#[test]
fn shadow_prisms_of_a_two_layer_wall_sweep_from_its_outer_faces() {
    let brushes = two_layer_wall();
    let classification = classify(&brushes).expect("classifies");
    for layer in [AFTER_HALL, AFTER_HALL + 1] {
        assert_eq!(
            classification.verdicts[layer],
            OccluderVerdict::Occludes,
            "precondition: layer {layer} occludes from its outer face"
        );
    }
    let prisms = prisms_of(&brushes);
    assert_eq!(prisms.len(), 2, "one from each outer face: {prisms:#?}");
    for (normal, face, depth) in [(DVec3::NEG_Z, -8.0, 8.0), (DVec3::Z, 8.3, 20.0 - 8.3)] {
        let prism = prism_on_face(&prisms, normal, face);
        assert!(
            (prism.depth - depth).abs() < MEASURE_EPS,
            "the prism ends at the hall wall: {} against {depth}",
            prism.depth
        );
        assert_splitters(
            prism,
            &[(DVec3::X, 8.0), (DVec3::NEG_X, -32.0)],
            "the composite wall's two ends",
        );
    }
}

#[test]
fn shadow_prism_offers_no_end_plane_to_a_region_reaching_into_it_under_the_spanning_tolerance() {
    // A plane splits only a region it spans by more than the region
    // classification tolerance, so a region reaching less than that into a
    // prism has no part inside it for the prism's end planes to bound.
    let tolerance = crate::partition::brush_bsp::REGION_CLASSIFICATION_EPSILON;
    let bound = bounded_wall_bound();
    let prisms = prisms_of(&bounded_wall_hall(bound + BOUND_STEP_M));
    let prism = prism_on_face(&prisms, DVec3::Z, WALL_BACK_Z);
    let region = |min: DVec3, max: DVec3| RegionPolytope::from_aabb(&Aabb { min, max });
    let far = WALL_BACK_Z + bound;
    for (reach, overlaps) in [(0.5 * tolerance, false), (2.0 * tolerance, true)] {
        let past_far_end = region(
            DVec3::new(0.0, 0.0, far - reach),
            DVec3::new(14.0, 6.0, far + 1.0),
        );
        assert_eq!(
            prism.overlaps(&past_far_end),
            overlaps,
            "a region reaching {reach} m into the far end"
        );
        let beside_end = region(
            DVec3::new(0.0, 0.0, WALL_BACK_Z),
            DVec3::new(3.0 + reach, 6.0, far - 1.0),
        );
        assert_eq!(
            prism.overlaps(&beside_end),
            overlaps,
            "a region reaching {reach} m across the end plane at x = 3"
        );
    }
}

#[test]
fn shadow_prisms_do_not_depend_on_brush_order() {
    // Turned walls, a chain with abutting ends, a cube offering every face,
    // walls meeting end to end, a wall in layers, a face counting by most of
    // its samples, a truncated prism, and an end at the free-probe threshold.
    let fixtures: Vec<(String, Vec<BrushVolume>)> = vec![
        ("retaining wall".into(), retaining_wall().brushes),
        ("chain".into(), chain_walls().brushes),
        ("10 m cube".into(), cube_in_room().brushes),
        ("walls in line".into(), in_line_walls()),
        ("two-layer wall".into(), two_layer_wall()),
        ("wall before a niche".into(), wall_before_niche(24.0)),
        (
            "bounded wall".into(),
            bounded_wall_hall(bounded_wall_bound() + 4.0),
        ),
        (
            "partly blocked end".into(),
            partly_blocked_end(
                FREE_END_PROBE_FRACTIONS.len() * FREE_END_PROBE_DEPTHS_M.len()
                    - FREE_END_MIN_OPEN_PROBES,
            ),
        ),
    ];
    for (name, brushes) in fixtures {
        let forward = prisms_of(&brushes);
        assert!(!forward.is_empty(), "{name}: precondition, some prism");
        let mut reversed = brushes.clone();
        reversed.reverse();
        let mut rotated = brushes.clone();
        rotated.rotate_left(brushes.len() / 2);
        for (order, permuted) in [("reversed", reversed), ("rotated", rotated)] {
            let other = prisms_of(&permuted);
            assert_eq!(forward.len(), other.len(), "{name} {order}: prism count");
            for (a, b) in forward.iter().zip(&other) {
                assert_eq!(a.truncated, b.truncated, "{name} {order}");
                assert!((a.depth - b.depth).abs() < MEASURE_EPS, "{name} {order}");
                assert!(
                    (a.face_normal() - b.face_normal()).length() < MEASURE_EPS
                        && (a.face_distance() - b.face_distance()).abs() < MEASURE_EPS,
                    "{name} {order}: same face in the same place"
                );
                assert_eq!(a.splitters.len(), b.splitters.len(), "{name} {order}");
                for (&(na, da), &(nb, db)) in a.splitters.iter().zip(&b.splitters) {
                    assert!(
                        (na - nb).length() < MEASURE_EPS && (da - db).abs() < MEASURE_EPS,
                        "{name} {order}: same splitters"
                    );
                }
            }
        }
    }
}
