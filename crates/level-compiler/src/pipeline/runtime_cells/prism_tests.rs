// Shadow-prism rows: which occluding faces sweep a prism, its edge and far
// planes, the length bound deciding both ways, order independence, and the
// far plane bounding an occluder-tier leaf.
// See: context/plans/in-progress/occluder-first-partition/index.md §Decisions

use glam::DVec3;

use super::{FirstPass, classify, partition_with_occluders};
use crate::map_data::BrushVolume;
use crate::partition::occluder_classify::test_fixtures::{
    box_brush, chain_walls, cube_in_room, hall, retaining_wall,
};
use crate::partition::occluder_classify::{OCCLUDER_FLOOR_M, OccluderVerdict};
use crate::partition::occluder_cluster::test_fixtures::strip_facade;
use crate::partition::{SHADOW_PRISM_LENGTH_FACTOR, ShadowPrism, shadow_prisms};

/// Tolerance on prism measures, which come from the first pass's segment
/// queries and own-frame boxes.
const MEASURE_EPS: f64 = 1e-6;

/// Step either side of the length bound in the rows that decide it both ways.
const BOUND_STEP_M: f64 = 0.25;

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

fn assert_has_splitter(prism: &ShadowPrism, (normal, distance): (DVec3, f64)) {
    assert!(
        prism.splitters.iter().any(|&(n, d)| {
            (n - normal).length() < MEASURE_EPS && (d - distance).abs() < MEASURE_EPS
        }),
        "prism lacks splitter {normal:?} · p = {distance}: {:?}",
        prism.splitters
    );
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
fn shadow_prism_of_a_retaining_wall_runs_from_each_broad_face_between_its_edge_planes() {
    // The wall spans x 13..39, y 0..6.5, z 30..30.81 in a hall 60 m long:
    // 29.19 m of open hall behind it and 30 m in front, both under twice
    // its 26 m length, so neither prism needs a far plane. The hall's slabs
    // occlude, but each fronts the void outside, where no eye stands, so
    // they sweep none: a room's own walls never cut the room.
    let fixture = retaining_wall();
    let prisms = prisms_of(&fixture.brushes);
    assert_eq!(prisms.len(), 2, "only the wall sweeps prisms: {prisms:#?}");
    for (normal, face, depth) in [(DVec3::Z, 30.81, 60.0 - 30.81), (DVec3::NEG_Z, -30.0, 30.0)] {
        let on_face = prisms_on_face(&prisms, normal, face);
        assert_eq!(on_face.len(), 1, "one prism on the face along {normal:?}");
        let prism = on_face[0];
        assert!(
            (prism.depth - depth).abs() < MEASURE_EPS,
            "the prism ends at the hall wall: {} against {depth}",
            prism.depth
        );
        assert!(!prism.truncated);
        assert_eq!(prism.splitters.len(), 4, "four edge planes, no far plane");
        for splitter in [
            (DVec3::X, 13.0),
            (DVec3::NEG_X, -39.0),
            (DVec3::Y, 0.0),
            (DVec3::NEG_Y, -6.5),
        ] {
            assert_has_splitter(prism, splitter);
        }
    }
}

#[test]
fn shadow_prism_length_bound_decides_both_ways_and_adds_a_far_plane_only_when_it_truncates() {
    let bound = bounded_wall_bound();
    let back_prism = |depth_behind: f64| {
        let prisms = prisms_of(&bounded_wall_hall(depth_behind));
        let on_face = prisms_on_face(&prisms, DVec3::Z, WALL_BACK_Z);
        assert_eq!(on_face.len(), 1, "one prism behind the wall");
        on_face[0].clone()
    };

    // Just inside the bound: the open space ends the prism.
    let inside = back_prism(bound - BOUND_STEP_M);
    assert!(!inside.truncated, "the open space ends the prism first");
    assert!((inside.depth - (bound - BOUND_STEP_M)).abs() < MEASURE_EPS);
    assert_eq!(inside.splitters.len(), 4, "no far plane");

    // Just outside it: the bound ends the prism, on a far plane.
    let outside = back_prism(bound + BOUND_STEP_M);
    assert!(outside.truncated, "the length bound ends the prism first");
    assert!((outside.depth - bound).abs() < MEASURE_EPS);
    assert_eq!(
        outside.splitters.len(),
        5,
        "the edge planes and a far plane"
    );
    assert_has_splitter(&outside, (DVec3::NEG_Z, -(WALL_BACK_Z + bound)));
}

#[test]
fn shadow_prism_far_plane_bounds_an_occluder_tier_leaf_only_when_the_bound_truncates() {
    let bound = bounded_wall_bound();
    let far = WALL_BACK_Z + bound;
    let tier_leaf_ends_at_far = |depth_behind: f64| {
        let brushes = bounded_wall_hall(depth_behind);
        let partition = partition_with_occluders(&brushes, &[]).expect("partition builds");
        partition.tier.leaves.iter().any(|leaf| {
            let b = &leaf.bounds;
            (b.min.z - far).abs() < MEASURE_EPS || (b.max.z - far).abs() < MEASURE_EPS
        })
    };
    assert!(
        !tier_leaf_ends_at_far(bound - BOUND_STEP_M),
        "inside the bound, nothing splits at the bound's distance"
    );
    assert!(
        tier_leaf_ends_at_far(bound + BOUND_STEP_M),
        "past the bound, the far plane bounds the space behind the wall"
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
fn shadow_prism_of_an_occluding_section_runs_from_its_bounding_face_only() {
    // The strip facade, x 15..46, y 0..15, slab z 20..20.3, fronts 20 m of
    // hall on -z and 19.7 m on +z: its bounding face is the -z one. Its
    // members are exempt alone, so no member face sweeps a prism.
    let cluster = strip_facade();
    let brushes = &cluster.fixture.brushes;
    let classification = classify(brushes).expect("classifies");
    let section = classification
        .clusters
        .iter()
        .flat_map(|cluster| &cluster.sections)
        .find(|section| section.occludes)
        .expect("the facade occludes as a section");
    assert!((section.bounding_face - DVec3::NEG_Z).length() < MEASURE_EPS);

    let prisms = prisms_of(brushes);
    let on_face = prisms_on_face(&prisms, DVec3::NEG_Z, -20.0);
    assert_eq!(on_face.len(), 1, "one prism on the bounding face");
    let prism = on_face[0];
    assert!((prism.depth - 20.0).abs() < MEASURE_EPS);
    assert!(!prism.truncated);
    for splitter in [
        (DVec3::X, 15.0),
        (DVec3::NEG_X, -46.0),
        (DVec3::Y, 0.0),
        (DVec3::NEG_Y, -15.0),
    ] {
        assert_has_splitter(prism, splitter);
    }
    assert!(
        prisms_on_face(&prisms, DVec3::Z, 20.3).is_empty(),
        "the other broad face sweeps none"
    );
    let in_slab = prisms.iter().filter(|prism| {
        let n = prism.face_normal();
        n.z.abs() > 1.0 - MEASURE_EPS && (19.0..21.5).contains(&prism.face_distance().abs())
    });
    assert_eq!(
        in_slab.count(),
        1,
        "no member face sweeps a prism of its own"
    );
}

#[test]
fn shadow_prisms_do_not_depend_on_brush_order() {
    // Turned walls, a chain, a cube offering every face, a section and a
    // truncated prism.
    let fixtures: Vec<(String, Vec<BrushVolume>)> = vec![
        ("retaining wall".into(), retaining_wall().brushes),
        ("chain".into(), chain_walls().brushes),
        ("10 m cube".into(), cube_in_room().brushes),
        ("strip facade".into(), strip_facade().fixture.brushes),
        (
            "bounded wall".into(),
            bounded_wall_hall(bounded_wall_bound() + 4.0),
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
