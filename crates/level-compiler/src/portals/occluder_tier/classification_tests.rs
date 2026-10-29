use std::collections::BTreeSet;

use super::classification_test_fixtures::{
    assert_apart, assert_pins, brush_at_quake_box, classification, classify, occlusion_test_brushes,
};
use crate::map_data::BrushVolume;
use crate::partition::occluder_classify::test_fixtures::{
    self as fixtures, Edge, Fixture, classification_fixtures, constant_edges,
};
use crate::partition::occluder_classify::{BrushFrame, OccluderVerdict};
use crate::partition::occluder_cluster::test_fixtures::cluster_fixtures;

// Classification table: each brush classifies as the Acceptance table shows.

#[test]
fn retaining_wall_occludes() {
    assert_pins(&fixtures::retaining_wall());
}

#[test]
fn stair_platform_stays_exempt_in_its_shaft() {
    assert_pins(&fixtures::stair_platform());
}

#[test]
fn floor_to_ceiling_column_stays_exempt_in_a_13_m_hall() {
    assert_pins(&fixtures::floor_to_ceiling_column());
}

#[test]
fn column_flush_in_a_niche_stays_exempt() {
    // The front face sees 1.5 of 13 m; the side faces front 0.25 m and the
    // back face none, so they do not count.
    assert_pins(&fixtures::column_in_niche());
}

#[test]
fn columns_down_a_corridor_stay_exempt() {
    assert_pins(&fixtures::columns_down_corridor());
}

#[test]
fn curb_stays_exempt() {
    assert_pins(&fixtures::curb());
}

#[test]
fn lamp_post_stays_exempt() {
    assert_pins(&fixtures::lamp_post());
}

#[test]
fn raised_street_deck_occludes_across_its_corridor() {
    assert_pins(&fixtures::street_deck());
}

#[test]
fn red_pillar_stays_exempt() {
    assert_pins(&fixtures::red_pillar());
}

// Cells (occluder-tier leaves) on either side of an occluder differ.

#[test]
fn two_sealed_rooms_occlude_slab_by_slab_and_locate_to_different_cells() {
    // Pin R30: each slab's own line runs past its ends into the void, but the
    // span in front of its inner face ends at the slabs it meets edge to edge.
    let fixture = fixtures::two_sealed_rooms();
    assert_pins(&fixture);
    assert_apart(&fixture);
}

#[test]
fn occlusion_test_first_room_walls_and_ceiling_occlude_alone() {
    // Pin R30 on the map itself. The east wall, a doorway's two jambs and its
    // lintel, is exempt brush by brush; the cluster rows judge it whole.
    let brushes = occlusion_test_brushes();
    let verdicts = classify(&brushes);
    for (label, min, max) in [
        ("ceiling", [-16.0, -16.0, 128.0], [272.0, 272.0, 144.0]),
        ("west wall", [-16.0, 0.0, 0.0], [0.0, 256.0, 128.0]),
        ("south wall", [0.0, -16.0, 0.0], [256.0, 0.0, 128.0]),
        ("north wall", [0.0, 256.0, 0.0], [256.0, 272.0, 128.0]),
    ] {
        let brush = brush_at_quake_box(&brushes, min, max);
        assert_eq!(
            verdicts[brush],
            OccluderVerdict::Occludes,
            "first room {label} (brush {brush})"
        );
    }

    let count = |verdict: OccluderVerdict| verdicts.iter().filter(|&&v| v == verdict).count();
    println!(
        "occlusion-test: {} brushes; occludes {}, under floor {}, no open face {}, \
         short span {}, low top {}",
        brushes.len(),
        count(OccluderVerdict::Occludes),
        count(OccluderVerdict::UnderFloor),
        count(OccluderVerdict::NoOpenFace),
        count(OccluderVerdict::ShortSpan),
        count(OccluderVerdict::LowTop),
    );
}

#[test]
fn room_in_a_hall_occludes_from_its_inner_faces_and_parts_from_the_hall() {
    // Pin R31: every outer face sees 8 of 40 m and fails; requiring both
    // faces would merge the room into the hall's cell.
    let fixture = fixtures::room_in_hall();
    assert_pins(&fixture);
    assert_apart(&fixture);
}

#[test]
fn open_air_maze_walls_occlude_and_part_neighbouring_corridors() {
    // The sky is far above, but each wall's top stands 3 m over the floor in
    // front of it; the 1 m railing stays under the floor.
    let fixture = fixtures::open_air_maze();
    assert_pins(&fixture);
    assert_apart(&fixture);
}

// Each constant decides both ways, and every test is inclusive.

#[test]
fn floor_decides_a_broad_axis_filling_its_span_both_ways() {
    assert_pins(&fixtures::floor_edge(Edge::Under));
    assert_pins(&fixtures::floor_edge(Edge::Over));
}

#[test]
fn absolute_length_decides_a_wall_under_half_its_span_both_ways() {
    assert_pins(&fixtures::absolute_length_edge(Edge::Under));
    assert_pins(&fixtures::absolute_length_edge(Edge::Over));
}

#[test]
fn half_span_decides_a_wall_under_the_absolute_length_both_ways() {
    assert_pins(&fixtures::half_span_edge(Edge::Under));
    assert_pins(&fixtures::half_span_edge(Edge::Over));
}

#[test]
fn see_over_height_decides_a_wall_top_both_ways() {
    assert_pins(&fixtures::see_over_edge(Edge::Under));
    assert_pins(&fixtures::see_over_edge(Edge::Over));
}

#[test]
fn floor_decides_the_gap_in_front_of_a_wall_passing_from_one_face_both_ways() {
    assert_pins(&fixtures::one_face_gap_edge(Edge::Under));
    assert_pins(&fixtures::one_face_gap_edge(Edge::Over));
}

#[test]
fn see_over_height_measures_from_a_top_that_sinks_into_a_roof() {
    // The room's roof closes the wall's top, so no one sees over it; the
    // measure still runs from the top, so a wall under the height fails.
    assert_pins(&fixtures::wall_under_roof(6.0));
    assert_pins(&fixtures::wall_under_roof(
        crate::partition::occluder_classify::OCCLUDER_SEE_OVER_M - 1e-3,
    ));
}

#[test]
fn every_occluder_test_passes_exactly_at_its_constant() {
    // Pin R12. The see-over drop starts 2 cm under the top and adds it back,
    // so a top exactly at the height measures exactly the height.
    for fixture in constant_edges(Edge::At) {
        assert_pins(&fixture);
    }
}

// Rotated brushes measure in their own frame (pin R5).

#[test]
fn curb_turned_45_degrees_stays_exempt_in_a_room_its_world_box_would_fill() {
    let fixture = fixtures::turned_curb();
    let curb = fixture.pins.last().expect("curb is pinned").brush;
    let frame = BrushFrame::of(&fixture.brushes[curb]).expect("curb has a frame");
    let mut extents = frame.extents;
    extents.sort_by(f64::total_cmp);
    for (got, want) in extents.iter().zip([0.3, 0.3, 25.0]) {
        assert!((got - want).abs() < 1e-9, "extents {extents:?}");
    }
    assert_pins(&fixture);
}

#[test]
fn retaining_wall_turned_45_degrees_hall_and_all_still_occludes() {
    assert_pins(&fixtures::turned_retaining_wall());
}

// Order invariance (pins R6, R18, R28).

/// Deterministic Fisher–Yates order of `0..n` from a fixed seed.
fn shuffled(n: usize, seed: u64) -> Vec<usize> {
    let mut state = seed;
    let mut order: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        order.swap(i, (state >> 33) as usize % (i + 1));
    }
    order
}

fn reversed(n: usize) -> Vec<usize> {
    (0..n).rev().collect()
}

/// Every order of `0..n`.
fn permutations(n: usize) -> Vec<Vec<usize>> {
    if n == 0 {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for rest in permutations(n - 1) {
        for slot in 0..=rest.len() {
            let mut order = rest.clone();
            order.insert(slot, n - 1);
            out.push(order);
        }
    }
    out
}

/// `brush` with its planes in `order`.
fn with_side_order(brush: &BrushVolume, order: &[usize]) -> BrushVolume {
    BrushVolume {
        planes: order.iter().map(|&i| brush.planes[i].clone()).collect(),
        ..brush.clone()
    }
}

/// What classification decides, in the original brush order: each brush's
/// verdict alone and whether it reaches the occluder tier, each cluster's
/// members, and each section's in-slab members and outcome.
#[derive(Debug, PartialEq)]
struct Outcome {
    verdicts: Vec<OccluderVerdict>,
    in_tier: Vec<bool>,
    clusters: BTreeSet<Vec<usize>>,
    /// In-slab members, whether the section occludes, and whether its
    /// bounding face faces along its normal.
    sections: BTreeSet<(Vec<usize>, bool, bool)>,
}

/// Classify `brushes` visited in `order`, each brush's sides in the order
/// `sides` gives for its plane count; the outcome names brushes by their
/// original index.
fn classify_reordered(
    brushes: &[BrushVolume],
    order: &[usize],
    sides: impl Fn(usize) -> Vec<usize>,
) -> Outcome {
    let reordered: Vec<BrushVolume> = order
        .iter()
        .map(|&i| with_side_order(&brushes[i], &sides(brushes[i].planes.len())))
        .collect();
    let visited = classification(&reordered);
    let in_tier = visited.occluder_flags();
    let original = |members: &[usize]| {
        let mut members: Vec<usize> = members.iter().map(|&p| order[p]).collect();
        members.sort_unstable();
        members
    };
    let mut outcome = Outcome {
        verdicts: vec![OccluderVerdict::UnderFloor; brushes.len()],
        in_tier: vec![false; brushes.len()],
        clusters: visited
            .clusters
            .iter()
            .map(|cluster| original(&cluster.members))
            .collect(),
        sections: visited
            .clusters
            .iter()
            .flat_map(|cluster| &cluster.sections)
            .map(|section| {
                (
                    original(&section.members),
                    section.occludes,
                    section.bounding_face.dot(section.normal) > 0.0,
                )
            })
            .collect(),
    };
    for (position, &brush) in order.iter().enumerate() {
        outcome.verdicts[brush] = visited.verdicts[position];
        outcome.in_tier[brush] = in_tier[position];
    }
    outcome
}

fn assert_order_free(name: &str, brushes: &[BrushVolume], orders: &[Vec<usize>]) {
    let identity = |n: usize| (0..n).collect::<Vec<_>>();
    let baseline = classify_reordered(brushes, &identity(brushes.len()), identity);
    for (k, order) in orders.iter().enumerate() {
        assert_eq!(
            classify_reordered(brushes, order, identity),
            baseline,
            "{name}: brush order {k}"
        );
        assert_eq!(
            classify_reordered(brushes, order, reversed),
            baseline,
            "{name}: brush order {k}, sides reversed"
        );
        assert_eq!(
            classify_reordered(brushes, order, |n| shuffled(n, k as u64 + 1)),
            baseline,
            "{name}: brush order {k}, sides shuffled"
        );
    }
}

#[test]
fn reordering_brushes_and_sides_changes_no_classification_or_cluster() {
    let fixtures = classification_fixtures()
        .into_iter()
        .chain(cluster_fixtures().into_iter().map(|cf| cf.fixture));
    for Fixture { name, brushes, .. } in fixtures {
        let n = brushes.len();
        let orders = [reversed(n), shuffled(n, 7), shuffled(n, 31)];
        assert_order_free(&name, &brushes, &orders);
    }
    let brushes = occlusion_test_brushes();
    assert_order_free(
        "occlusion-test",
        &brushes,
        &[reversed(brushes.len()), shuffled(brushes.len(), 7)],
    );
}

#[test]
fn cube_classifies_the_same_under_every_side_order() {
    // Pin R18: every face ties for broad, so the frame and the faces tried
    // must not depend on which side comes first.
    let fixture = fixtures::cube_in_room();
    let cube = fixture.pins.last().expect("cube is pinned").brush;
    for order in permutations(fixture.brushes[cube].planes.len()) {
        let mut brushes = fixture.brushes.clone();
        brushes[cube] = with_side_order(&brushes[cube], &order);
        assert_eq!(
            classify(&brushes)[cube],
            OccluderVerdict::Occludes,
            "side order {order:?}"
        );
    }
}

#[test]
fn near_cube_seeds_the_same_frame_and_verdict_under_every_side_order() {
    // Pin R18 off true: each ordered pair of near-perpendicular sides seeds
    // its own frame, a hair apart, so which pairs are tried must not depend
    // on which side comes first.
    let fixture = fixtures::near_cube_in_room();
    let cube = fixture.pins.last().expect("near-cube is pinned").brush;
    let baseline = BrushFrame::of(&fixture.brushes[cube]).expect("near-cube has a frame");
    for order in permutations(fixture.brushes[cube].planes.len()) {
        let mut brushes = fixture.brushes.clone();
        brushes[cube] = with_side_order(&brushes[cube], &order);
        let frame = BrushFrame::of(&brushes[cube]).expect("near-cube has a frame");
        // The axes come from the planes alone; the extents and centre also
        // carry the rounding of the corners clipped in this side order.
        assert_eq!(frame.axes, baseline.axes, "side order {order:?}");
        assert!(
            (frame.center - baseline.center).abs().max_element() < 1e-9
                && frame
                    .extents
                    .iter()
                    .zip(baseline.extents)
                    .all(|(got, want)| (got - want).abs() < 1e-9),
            "side order {order:?}: {frame:?} vs {baseline:?}"
        );
        assert_eq!(
            classify(&brushes)[cube],
            OccluderVerdict::Occludes,
            "side order {order:?}"
        );
    }
}

#[test]
fn wall_leaning_back_against_solid_occludes_from_its_face() {
    // Its back is buried in the hall wall, so its face alone decides, and
    // the see-over drop must clear the face's own foot.
    assert_pins(&fixtures::leaning_wall(fixtures::LEAN_RAD));
}

#[test]
fn chain_walls_occlude_in_every_brush_order() {
    // Pin R28: each 12 m wall's span ends at the wall it abuts, whichever
    // brush the classifier meets first.
    let fixture = fixtures::chain_walls();
    let chain: Vec<usize> = fixture.pins.iter().map(|pin| pin.brush).collect();
    let hall: Vec<usize> = (0..fixture.brushes.len())
        .filter(|brush| !chain.contains(brush))
        .collect();
    for chain_order in permutations(chain.len()) {
        let walls: Vec<usize> = chain_order.iter().map(|&i| chain[i]).collect();
        for order in [
            hall.iter().chain(&walls).copied().collect::<Vec<_>>(),
            walls.iter().chain(&hall).copied().collect(),
        ] {
            let verdicts =
                classify_reordered(&fixture.brushes, &order, |n| (0..n).collect::<Vec<_>>())
                    .verdicts;
            for pin in &fixture.pins {
                assert_eq!(
                    verdicts[pin.brush],
                    OccluderVerdict::Occludes,
                    "{} in order {order:?}",
                    pin.label
                );
            }
        }
    }
}
