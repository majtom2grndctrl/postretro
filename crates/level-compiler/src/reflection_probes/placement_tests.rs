// Reflection-probe placement contracts: derived groups and fits, capture points, drops, binding and the cap.
// See: context/lib/resource_management.md §5.1

use std::collections::HashSet;

use glam::{DVec2, DVec3};
use log::Level;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_test_log_capture::LogCapture;

use crate::bake_control::BakeControl;
use crate::bvh_build::build_bvh;
use crate::map_data::MapReflectionVolume;
use crate::partition::BspTree;

use super::box_fit::WorldBounds;
use super::capture_point::{
    CAPTURE_CLEARANCE_M, NEAREST_POINT_INSET_M, PointClassifier, PointFault,
    nearest_point_on_polygon,
};
use super::derived::FIT_MATCH_TOLERANCE_M;
use super::hit::SceneTracer;
use super::placement::{
    CAPTURE_LIFT_M, PlacedProbe, PlacementScene, UpFacingFace, bindings, place_probes,
};
use super::placement_test_fixtures::{
    PlacementFixture, UNIT, box_brush, engine, engine_box, fluid, reflection_volume, sealed_room,
};
use super::reflection_probes_test_fixtures::{box_room, fluid_volume, up_rect, volumes};

const ROOM_MIN: [f64; 3] = [0.0, 0.0, 0.0];
const ROOM_MAX: [f64; 3] = [512.0, 256.0, 192.0];
/// A reflection volume filling the room, a unit in from its walls.
const ROOM_BOX: ([f64; 3], [f64; 3]) = ([8.0, 8.0, 0.0], [504.0, 248.0, 184.0]);

fn room() -> Vec<String> {
    sealed_room(ROOM_MIN, ROOM_MAX)
}

fn room_with(extra: &[String]) -> Vec<String> {
    let mut world = room();
    world.extend(extra.iter().cloned());
    world
}

fn water(brushes: &[([f64; 3], [f64; 3])]) -> String {
    let brushes: Vec<String> = brushes.iter().map(|&(a, b)| box_brush(a, b)).collect();
    fluid("water", &brushes)
}

fn probe_binding<'p>(probes: &'p [PlacedProbe], face: &UpFacingFace) -> Option<&'p PlacedProbe> {
    probes
        .iter()
        .find(|p| p.faces.contains(&(face.volume, face.face)))
}

/// The up-facing face whose area centroid is nearest the map-unit point.
fn face_near(faces: &[UpFacingFace], q: [f64; 3]) -> &UpFacingFace {
    let target = engine(q);
    faces
        .iter()
        .min_by(|a, b| {
            let da = (a.geometry.centroid - target).length();
            let db = (b.geometry.centroid - target).length();
            da.total_cmp(&db)
        })
        .expect("the fixture has up-facing faces")
}

fn point(probe: &PlacedProbe) -> DVec3 {
    DVec3::from(probe.capture_point.map(f64::from))
}

#[track_caller]
fn assert_near(actual: DVec3, expected: DVec3, tolerance: f64) {
    assert!(
        (actual - expected).abs().max_element() <= tolerance,
        "{actual} is not within {tolerance} of {expected}"
    );
}

/// The box sides a fit can bound: the walls and the ceiling. Its floor is the
/// capture plane.
#[track_caller]
fn assert_fits(lo: DVec3, hi: DVec3, room: (DVec3, DVec3), tolerance: f64) {
    for (actual, expected) in [
        (lo.x, room.0.x),
        (lo.z, room.0.z),
        (hi.x, room.1.x),
        (hi.z, room.1.z),
        (hi.y, room.1.y),
    ] {
        assert!(
            (actual - expected).abs() <= tolerance,
            "box {lo}..{hi} is not within {tolerance} of the room {:?}",
            room
        );
    }
}

fn probe_box(probe: &PlacedProbe) -> (DVec3, DVec3) {
    (
        DVec3::from(probe.box_min.map(f64::from)),
        DVec3::from(probe.box_max.map(f64::from)),
    )
}

/// Horizontal distance from `p` to the nearest of `faces`' water; 0 over it.
fn distance_to_water(p: DVec3, faces: &[&UpFacingFace]) -> f64 {
    faces
        .iter()
        .map(|face| nearest_point_on_polygon(DVec2::new(p.x, p.z), &face.outline).1)
        .fold(f64::INFINITY, f64::min)
}

// ---------------------------------------------------------------- derived

// P17: a box room with a pool built from three coplanar brushes of one entity.
#[test]
fn derived_faces_of_one_pool_fit_the_room_and_share_one_probe() {
    let fixture = PlacementFixture::compile(
        &room(),
        &[water(&[
            ([64.0, 64.0, 0.0], [192.0, 192.0, 32.0]),
            ([192.0, 64.0, 0.0], [320.0, 192.0, 32.0]),
            ([320.0, 64.0, 0.0], [448.0, 192.0, 32.0]),
        ])],
    );
    let faces = fixture.up_faces();
    assert_eq!(faces.len(), 3, "one top per brush");
    let room = engine_box(ROOM_MIN, ROOM_MAX);
    for face in &faces {
        let (lo, hi) = fixture.face_fit(face);
        assert_fits(lo, hi, room, FIT_MATCH_TOLERANCE_M);
    }
    let probes = fixture.place().unwrap();
    assert_eq!(probes.len(), 1, "{probes:?}");
    let probe = &probes[0];
    assert_eq!(probe.source_entity, 1);
    assert_eq!(probe.faces.len(), 3);
    let (lo, hi) = probe_box(probe);
    assert_fits(lo, hi, room, FIT_MATCH_TOLERANCE_M);
    // The group's area centroid, 2 mm above the water, inside the union box.
    let water = 32.0 * UNIT;
    assert_near(
        point(probe),
        engine([256.0, 128.0, 0.0]) + DVec3::Y * (water + CAPTURE_LIFT_M),
        1e-4,
    );
    assert!(point(probe).cmpge(lo).all() && point(probe).cmple(hi).all());
    assert!((lo.y - (water + CAPTURE_LIFT_M)).abs() < 1e-4, "{lo}");
}

// P30: a pool sunk below its room's floor fits the room, not the pit.
#[test]
fn sunken_pool_fits_its_room_and_captures_over_the_basin() {
    let (floor, top) = (-112.0, -16.0);
    let mut world = sealed_room([0.0, 0.0, floor], ROOM_MAX);
    for (a, b) in [
        ([0.0, 0.0, floor], [192.0, 256.0, 0.0]),
        ([320.0, 0.0, floor], [512.0, 256.0, 0.0]),
        ([192.0, 0.0, floor], [320.0, 64.0, 0.0]),
        ([192.0, 192.0, floor], [320.0, 256.0, 0.0]),
    ] {
        world.push(box_brush(a, b));
    }
    let fixture = PlacementFixture::compile(
        &world,
        &[water(&[([192.0, 64.0, floor], [320.0, 192.0, top])])],
    );
    let probes = fixture.place().unwrap();
    assert_eq!(probes.len(), 1, "{probes:?}");
    let (lo, hi) = probe_box(&probes[0]);
    assert_fits(
        lo,
        hi,
        engine_box(ROOM_MIN, ROOM_MAX),
        FIT_MATCH_TOLERANCE_M,
    );
    let basin = engine_box([192.0, 64.0, floor], [320.0, 192.0, top]);
    let p = point(&probes[0]);
    assert!(p.x > basin.0.x && p.x < basin.1.x && p.z > basin.0.z && p.z < basin.1.z);
    assert!((p.y - (top * UNIT + CAPTURE_LIFT_M)).abs() < 1e-4, "{p}");
}

#[test]
fn faces_of_one_entity_at_two_heights_get_separate_probes() {
    let fixture = PlacementFixture::compile(
        &room(),
        &[water(&[
            ([64.0, 64.0, 0.0], [192.0, 192.0, 32.0]),
            ([256.0, 64.0, 0.0], [384.0, 192.0, 48.0]),
        ])],
    );
    let faces = fixture.up_faces();
    let probes = fixture.place().unwrap();
    assert_eq!(probes.len(), 2, "{probes:?}");
    let low = probe_binding(&probes, face_near(&faces, [128.0, 128.0, 32.0])).unwrap();
    let high = probe_binding(&probes, face_near(&faces, [320.0, 128.0, 48.0])).unwrap();
    assert_ne!(low, high);
    assert!((point(low).y - (32.0 * UNIT + CAPTURE_LIFT_M)).abs() < 1e-4);
    assert!((point(high).y - (48.0 * UNIT + CAPTURE_LIFT_M)).abs() < 1e-4);
    for probe in [low, high] {
        let (lo, hi) = probe_box(probe);
        assert_fits(
            lo,
            hi,
            engine_box(ROOM_MIN, ROOM_MAX),
            FIT_MATCH_TOLERANCE_M,
        );
    }
}

/// Two 240 u rooms joined by a 64 u doorway in a 32 u wall at x = 0.
fn doorway_rooms() -> Vec<String> {
    let mut world = sealed_room([-256.0, -128.0, 0.0], [256.0, 128.0, 128.0]);
    for (a, b) in [
        ([-16.0, -128.0, 0.0], [16.0, -32.0, 128.0]),
        ([-16.0, 32.0, 0.0], [16.0, 128.0, 128.0]),
        ([-16.0, -32.0, 96.0], [16.0, 32.0, 128.0]),
    ] {
        world.push(box_brush(a, b));
    }
    world
}

#[test]
fn one_height_in_two_rooms_joined_by_a_doorway_gets_a_probe_per_room() {
    let fixture = PlacementFixture::compile(
        &doorway_rooms(),
        &[water(&[
            ([-200.0, -48.0, 0.0], [-80.0, 48.0, 16.0]),
            ([80.0, -48.0, 0.0], [200.0, 48.0, 16.0]),
        ])],
    );
    let faces = fixture.up_faces();
    let probes = fixture.place().unwrap();
    assert_eq!(probes.len(), 2, "{probes:?}");
    let west = probe_binding(&probes, face_near(&faces, [-140.0, 0.0, 16.0])).unwrap();
    let east = probe_binding(&probes, face_near(&faces, [140.0, 0.0, 16.0])).unwrap();
    assert_ne!(west, east);
    let (lo, hi) = probe_box(west);
    assert_fits(
        lo,
        hi,
        engine_box([-256.0, -128.0, 0.0], [-16.0, 128.0, 128.0]),
        FIT_MATCH_TOLERANCE_M,
    );
    let (lo, hi) = probe_box(east);
    assert_fits(
        lo,
        hi,
        engine_box([16.0, -128.0, 0.0], [256.0, 128.0, 128.0]),
        FIT_MATCH_TOLERANCE_M,
    );
}

// P32: a derived group ringing a pillar, and an L-shaped pool whose
// centroid falls in the notch. The pillar is slim beside the water's reach,
// so every piece around it still fits the room; a broad one standing close
// to the pieces is a wall to each, and splits the ring into a probe per side.
#[test]
fn derived_centroid_off_its_water_moves_to_the_nearest_point_on_it() {
    let pillar = ([248.0, 120.0, 0.0], [264.0, 136.0, 192.0]);
    let fixture = PlacementFixture::compile(
        &room_with(&[box_brush(pillar.0, pillar.1)]),
        &[water(&[([64.0, 32.0, 0.0], [448.0, 224.0, 32.0])])],
    );
    let faces = fixture.up_faces();
    assert!(faces.len() > 1, "the pillar cuts the top into pieces");
    let probes = fixture.place().unwrap();
    assert_eq!(probes.len(), 1, "{probes:?}");
    let p = point(&probes[0]);
    let all: Vec<&UpFacingFace> = faces.iter().collect();
    assert!(distance_to_water(p, &all) <= 1e-6, "{p} is over water");
    // Inset from the pillar's wall, not inside it.
    let (lo, hi) = engine_box(pillar.0, pillar.1);
    let outside = DVec2::new(
        (lo.x - p.x).max(p.x - hi.x).max(0.0),
        (lo.z - p.z).max(p.z - hi.z).max(0.0),
    )
    .length();
    assert!(
        (outside - NEAREST_POINT_INSET_M).abs() < 1e-4,
        "{outside} m from the pillar"
    );
    assert!((p.y - (32.0 * UNIT + CAPTURE_LIFT_M)).abs() < 1e-4);

    let fixture = PlacementFixture::compile(
        &room(),
        &[water(&[
            ([64.0, 32.0, 0.0], [320.0, 64.0, 32.0]),
            ([64.0, 64.0, 0.0], [96.0, 224.0, 32.0]),
        ])],
    );
    let faces = fixture.up_faces();
    let probes = fixture.place().unwrap();
    assert_eq!(probes.len(), 1, "one L-shaped group: {probes:?}");
    let p = point(&probes[0]);
    let all: Vec<&UpFacingFace> = faces.iter().collect();
    assert!(distance_to_water(p, &all) <= 1e-6, "{p} is over water");
    // The area centroid itself lies in the notch.
    let mut weighted = DVec3::ZERO;
    let mut area = 0.0;
    for face in &faces {
        weighted += face.geometry.centroid * face.geometry.area;
        area += face.geometry.area;
    }
    assert!(distance_to_water(weighted / area, &all) > 0.5);
}

// P15: a derived point in solid, inside another fluid, or outside the hull
// drops with a warning naming the fluid entity; its faces bind nothing and
// take no room under the cap.
#[test]
fn derived_probe_with_an_invalid_capture_point_drops_naming_its_fluid_entity() {
    let capture = LogCapture::start();
    // A block hanging 1.5 mm above the water, over the pool's centroid: not
    // flush, so the water under it stays, and the point 2 mm up is inside it.
    let block = box_brush([112.0, 112.0, 32.059], [144.0, 144.0, 192.0]);
    let fixture = PlacementFixture::compile(
        &room_with(&[block]),
        &[
            water(&[([64.0, 64.0, 0.0], [192.0, 192.0, 32.0])]),
            water(&[([320.0, 64.0, 0.0], [448.0, 192.0, 32.0])]),
        ],
    );
    let probes = fixture.place_with_cap(1).unwrap();
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0].source_entity, 2, "the valid pool keeps its probe");
    capture.assert_logged_once(Level::Warn, "derived probe of fluid entity 1");
    capture.assert_logged_once(Level::Warn, "is in solid; dropped");
    capture.assert_not_logged(Level::Warn, "past the 1-probe cap");
    let bound = bindings(&probes);
    assert!(bound.iter().all(|b| b.volume == 1), "{bound:?}");

    capture.clear();
    let fixture = PlacementFixture::compile(
        &room(),
        &[
            water(&[([64.0, 64.0, 0.0], [192.0, 192.0, 32.0])]),
            fluid(
                "sludge",
                &[box_brush([32.0, 32.0, 0.0], [224.0, 224.0, 64.0])],
            ),
        ],
    );
    let probes = fixture.place().unwrap();
    assert!(probes.iter().all(|p| p.source_entity == 2), "{probes:?}");
    capture.assert_logged_once(Level::Warn, "is inside fluid entity 2; dropped");

    capture.clear();
    let fixture = PlacementFixture::compile(
        &room(),
        &[water(&[([600.0, 64.0, 0.0], [700.0, 192.0, 32.0])])],
    );
    assert!(fixture.place().unwrap().is_empty());
    capture.assert_logged_once(Level::Warn, "is outside the hull; dropped");
}

#[test]
fn a_valid_probe_beside_a_wall_is_kept() {
    let capture = LogCapture::start();
    let fixture = PlacementFixture::compile(
        &room(),
        &[water(&[([64.0, 0.0, 0.0], [448.0, 16.0, 32.0])])],
    );
    let probes = fixture.place().unwrap();
    assert_eq!(probes.len(), 1);
    assert_near(
        point(&probes[0]),
        engine([256.0, 8.0, 0.0]) + DVec3::Y * (32.0 * UNIT + CAPTURE_LIFT_M),
        1e-4,
    );
    capture.assert_not_logged(Level::Warn, "[ReflectionProbes]");
}

// A derived centroid 5 mm from a wall, closer than the clearance, moves the
// clearance off it and keeps its probe: a capture ray would otherwise start
// within reach of the wall, but the probe is valid.
#[test]
fn a_derived_centroid_within_the_clearance_of_a_wall_moves_clear_and_is_kept() {
    let capture = LogCapture::start();
    // A block hanging 1.5 mm above the water, its west face 0.2 units
    // (5.08 mm) east of the pool's centroid at x = 128: the water runs under
    // it, and the point 2 mm up stands level with it.
    let block = box_brush([128.2, 112.0, 32.059], [160.0, 144.0, 192.0]);
    let fixture = PlacementFixture::compile(
        &room_with(&[block]),
        &[water(&[([64.0, 64.0, 0.0], [192.0, 192.0, 32.0])])],
    );
    let faces = fixture.up_faces();
    let probes = fixture.place().unwrap();
    assert_eq!(probes.len(), 1, "{probes:?}");
    let p = point(&probes[0]);
    // West of the face is higher engine z: moved the clearance west.
    let centroid = engine([128.0, 128.0, 0.0]) + DVec3::Y * (32.0 * UNIT + CAPTURE_LIFT_M);
    assert_near(p, centroid + DVec3::Z * CAPTURE_CLEARANCE_M, 1e-5);
    let face_z = engine([128.2, 0.0, 0.0]).z;
    assert!(p.z - face_z >= CAPTURE_CLEARANCE_M, "{p}");
    let all: Vec<&UpFacingFace> = faces.iter().collect();
    assert!(distance_to_water(p, &all) <= 1e-6, "{p} is over water");
    let (lo, hi) = probe_box(&probes[0]);
    assert!(p.cmpge(lo).all() && p.cmple(hi).all(), "{p} in {lo}..{hi}");
    let classifier = PointClassifier::new(&fixture.tree, &fixture.exterior, &fixture.environment);
    assert_eq!(classifier.fault(p), None);
    assert_eq!(
        classifier.fault(centroid),
        Some(PointFault::Crowded),
        "crowded where it started"
    );
    capture.assert_not_logged(Level::Warn, "[ReflectionProbes]");
}

// ---------------------------------------------------------------- authored

// P33: a room box over a main pool and a lower basin, then a smaller box
// around the basin.
#[test]
fn authored_box_over_two_heights_captures_over_the_lowest_and_warns() {
    let capture = LogCapture::start();
    let pools = water(&[
        ([64.0, 64.0, 0.0], [256.0, 192.0, 32.0]),
        ([320.0, 64.0, 0.0], [448.0, 192.0, 16.0]),
    ]);
    let room_box = reflection_volume("", ROOM_BOX.0, ROOM_BOX.1);
    let fixture = PlacementFixture::compile(&room(), &[pools.clone(), room_box.clone()]);
    let probes = fixture.place().unwrap();
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0].source_entity, 2);
    assert_eq!(probes[0].faces.len(), 2, "both planes bind the room box");
    let basin_point = engine([384.0, 128.0, 0.0]) + DVec3::Y * (16.0 * UNIT + CAPTURE_LIFT_M);
    assert_near(point(&probes[0]), basin_point, 1e-4);
    capture.assert_logged_once(
        Level::Warn,
        "entity 2 'reflection_volume' binds water at more than one height (0.406, 0.813 m)",
    );

    capture.clear();
    let basin_box = reflection_volume("", [304.0, 48.0, 0.0], [464.0, 208.0, 64.0]);
    let fixture = PlacementFixture::compile(&room(), &[pools, room_box, basin_box]);
    let faces = fixture.up_faces();
    let probes = fixture.place().unwrap();
    assert_eq!(
        probes.iter().map(|p| p.source_entity).collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert_near(point(&probes[0]), basin_point, 1e-4);
    let main = face_near(&faces, [160.0, 128.0, 32.0]);
    let basin = face_near(&faces, [384.0, 128.0, 16.0]);
    assert_eq!(probes[0].faces, vec![(main.volume, main.face)]);
    assert_eq!(probes[1].faces, vec![(basin.volume, basin.face)]);
    capture.assert_not_logged(Level::Warn, "more than one height");
}

// P29: a face counts toward and binds the box holding its area centroid,
// however far it reaches past the box's wall.
#[test]
fn straddling_face_counts_and_binds_where_its_centroid_lies() {
    let capture = LogCapture::start();
    let pool = water(&[([64.0, 64.0, 0.0], [448.0, 192.0, 32.0])]);
    let fixture = PlacementFixture::compile(
        &room(),
        &[
            pool.clone(),
            reflection_volume("", [0.0, 0.0, 0.0], [300.0, 256.0, 128.0]),
        ],
    );
    let probes = fixture.place().unwrap();
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0].source_entity, 2);
    assert_near(
        point(&probes[0]),
        engine([256.0, 128.0, 0.0]) + DVec3::Y * (32.0 * UNIT + CAPTURE_LIFT_M),
        1e-4,
    );

    // Most of the face under the box, its centroid outside: neither counts
    // nor binds, so the box drops and the face takes a derived probe.
    let fixture = PlacementFixture::compile(
        &room(),
        &[
            pool,
            reflection_volume("", [0.0, 0.0, 0.0], [240.0, 256.0, 128.0]),
        ],
    );
    let probes = fixture.place().unwrap();
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0].source_entity, 1, "derived");
    capture.assert_logged_once(
        Level::Warn,
        "entity 2 'reflection_volume' binds no up-facing fluid face; dropped",
    );
}

// P32, authored: lowest-plane faces ringing an island.
#[test]
fn authored_centroid_over_an_island_moves_onto_the_water_and_is_kept() {
    let pillar = ([224.0, 96.0, 0.0], [288.0, 160.0, 192.0]);
    let fixture = PlacementFixture::compile(
        &room_with(&[box_brush(pillar.0, pillar.1)]),
        &[
            water(&[([128.0, 32.0, 0.0], [384.0, 224.0, 32.0])]),
            reflection_volume("", ROOM_BOX.0, ROOM_BOX.1),
        ],
    );
    let faces = fixture.up_faces();
    let probes = fixture.place().unwrap();
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0].source_entity, 2);
    let p = point(&probes[0]);
    let all: Vec<&UpFacingFace> = faces.iter().collect();
    assert!(distance_to_water(p, &all) <= 1e-6, "{p}");
    let (lo, hi) = engine_box(pillar.0, pillar.1);
    assert!(p.x < lo.x || p.x > hi.x || p.z < lo.z || p.z > hi.z, "{p}");
}

// An authored point in a pillar standing in the water drops; with
// `_capture_offset` clear of the pillar it is kept. The pillar hangs 1.5 mm
// above the water, so the water runs under it and its centroid is over
// water: a pillar that cuts the water instead moves the point onto the
// water (P32) before any drop test.
#[test]
fn authored_point_in_a_pillar_drops_and_an_offset_clear_of_it_is_kept() {
    let capture = LogCapture::start();
    let pillar = box_brush([112.0, 112.0, 32.059], [144.0, 144.0, 192.0]);
    let pool = water(&[([64.0, 64.0, 0.0], [192.0, 192.0, 32.0])]);
    let box_min = [48.0, 48.0, 0.0];
    let box_max = [216.0, 208.0, 96.0];
    let fixture = PlacementFixture::compile(
        &room_with(std::slice::from_ref(&pillar)),
        &[pool.clone(), reflection_volume("", box_min, box_max)],
    );
    let probes = fixture.place().unwrap();
    assert!(probes.iter().all(|p| p.source_entity != 2), "{probes:?}");
    capture.assert_logged_once(Level::Warn, "entity 2 'reflection_volume' capture point");
    capture.assert_logged(Level::Warn, "is in solid; dropped");

    capture.clear();
    let fixture = PlacementFixture::compile(
        &room_with(&[pillar]),
        &[
            pool,
            reflection_volume("\"_capture_offset\" \"48 0\"", box_min, box_max),
        ],
    );
    let probes = fixture.place().unwrap();
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0].source_entity, 2);
    assert_near(
        point(&probes[0]),
        engine([176.0, 128.0, 0.0]) + DVec3::Y * (32.0 * UNIT + CAPTURE_LIFT_M),
        1e-4,
    );
    capture.assert_not_logged(Level::Warn, "[ReflectionProbes]");
}

// Capture rays ignore hits within the tracer's 1 mm epsilon, so an authored
// `_capture_offset` that sets the point on a wall, or a fraction of a
// millimetre beside it, would capture through the wall. A point inside the
// wall drops as in solid; one beside it, closer than the clearance, moves
// the clearance off it and is kept; one on the wall plane does one or the
// other, by the side the BSP gives the plane. Whichever, no kept point
// starts its capture on the wall.
#[test]
fn an_offset_point_in_a_wall_drops_and_one_just_beside_it_moves_clear() {
    let capture = LogCapture::start();
    // A block hanging 1.5 mm above the water, over the pool's centroid: the
    // water runs under it, and the point 2 mm up stands level with it.
    let block = box_brush([112.0, 112.0, 32.059], [144.0, 144.0, 192.0]);
    let pool = water(&[([64.0, 64.0, 0.0], [192.0, 192.0, 32.0])]);
    let world = room_with(&[block]);
    let fixture_with = |offset: &str| {
        let properties = format!("\"_capture_offset\" \"{offset}\"");
        PlacementFixture::compile(
            &world,
            &[
                pool.clone(),
                reflection_volume(&properties, [48.0, 48.0, 0.0], [216.0, 208.0, 96.0]),
            ],
        )
    };
    // The block's east face, x = 144, is engine z = −144 units; east of it
    // is lower z.
    let face_z = engine([144.0, 0.0, 0.0]).z;
    let water_y = 32.0 * UNIT + CAPTURE_LIFT_M;
    let kept_clear = |offset: &str| -> Option<DVec3> {
        let fixture = fixture_with(offset);
        let probes = fixture.place().unwrap();
        let probe = probes.iter().find(|p| p.source_entity == 2)?;
        let p = point(probe);
        let clearance = face_z - p.z;
        assert!(
            clearance >= CAPTURE_CLEARANCE_M - 1e-6,
            "{offset}: {p} is {clearance} m off the wall"
        );
        let classifier =
            PointClassifier::new(&fixture.tree, &fixture.exterior, &fixture.environment);
        assert_eq!(classifier.fault(p), None, "{offset}: {p}");
        assert_near(
            DVec3::new(p.x, p.y, 0.0),
            DVec3::new(engine([0.0, 128.0, 0.0]).x, water_y, 0.0),
            1e-5,
        );
        Some(p)
    };

    // 0.5 mm inside the block.
    capture.clear();
    assert_eq!(kept_clear("15.98 0"), None);
    capture.assert_logged_once(Level::Warn, "entity 2 'reflection_volume' capture point");
    capture.assert_logged(Level::Warn, "is in solid; dropped");

    // On the face: dropped as in solid, or kept clear of it.
    capture.clear();
    if kept_clear("16 0").is_none() {
        capture.assert_logged(Level::Warn, "is in solid; dropped");
    } else {
        capture.assert_not_logged(Level::Warn, "[ReflectionProbes]");
    }

    // 0.5 mm east of the face: moved the clearance east, so 10.5 mm off it.
    capture.clear();
    let p = kept_clear("16.02 0").expect("a point beside the wall is kept");
    assert!(
        (face_z - p.z - (0.02 * UNIT + CAPTURE_CLEARANCE_M)).abs() < 1e-5,
        "{p}"
    );
    capture.assert_not_logged(Level::Warn, "[ReflectionProbes]");

    // 1.27 cm clear: kept where the offset puts it.
    capture.clear();
    let p = kept_clear("16.5 0").expect("a point clear of the wall is kept");
    assert!((p.z - engine([144.5, 0.0, 0.0]).z).abs() < 1e-5, "{p}");
    capture.assert_not_logged(Level::Warn, "[ReflectionProbes]");
}

// A point beside a wall, closer than the clearance, whose move off it would
// leave its box, drops; it is in open space, so its warning says it cannot
// clear the wall rather than that it is in solid.
#[test]
fn an_offset_point_that_cannot_clear_a_wall_within_its_box_drops_saying_so() {
    let capture = LogCapture::start();
    // The block of the test above; the point 0.5 mm east of its east face,
    // x = 144, and the box's east wall 2.5 mm past the point, short of the
    // 1 cm move.
    let block = box_brush([112.0, 112.0, 32.059], [144.0, 144.0, 192.0]);
    let fixture = PlacementFixture::compile(
        &room_with(&[block]),
        &[
            water(&[([64.0, 64.0, 0.0], [192.0, 192.0, 32.0])]),
            reflection_volume(
                "\"_capture_offset\" \"16.02 0\"",
                [48.0, 48.0, 0.0],
                [144.1, 208.0, 96.0],
            ),
        ],
    );
    let probes = fixture.place().unwrap();
    assert!(probes.iter().all(|p| p.source_entity != 2), "{probes:?}");
    capture.assert_logged_once(Level::Warn, "entity 2 'reflection_volume' capture point");
    capture.assert_logged_once(
        Level::Warn,
        "is too close to a wall to move clear of it within its box and over its water; dropped",
    );
    // Only the derived probe its faces fall to, whose point is inside the
    // block, is in solid.
    capture.assert_logged_once(Level::Warn, "derived probe of fluid entity 1");
    capture.assert_logged_once(Level::Warn, "is in solid; dropped");
}

// Acceptance row 5, authored half of the in-fluid and outside-hull drops:
// an authored box whose capture point lies inside another fluid entity, or
// outside the hull, drops with a warning naming the reflection volume; its
// faces then fall to their derived probes, which drop for the same reason.
#[test]
fn authored_point_inside_another_fluid_or_outside_the_hull_drops_naming_the_entity() {
    let capture = LogCapture::start();
    let fixture = PlacementFixture::compile(
        &room(),
        &[
            water(&[([64.0, 64.0, 0.0], [192.0, 192.0, 32.0])]),
            fluid(
                "sludge",
                &[box_brush([32.0, 32.0, 0.0], [224.0, 224.0, 64.0])],
            ),
            reflection_volume("", [48.0, 48.0, 0.0], [216.0, 208.0, 96.0]),
        ],
    );
    // Only the sludge's own top keeps a (derived) probe.
    let probes = fixture.place().unwrap();
    assert!(probes.iter().all(|p| p.source_entity == 2), "{probes:?}");
    capture.assert_logged_once(Level::Warn, "entity 3 'reflection_volume' capture point");
    capture.assert_logged(Level::Warn, "is inside fluid entity 2; dropped");
    capture.assert_not_logged(Level::Warn, "binds no up-facing fluid face");

    capture.clear();
    let fixture = PlacementFixture::compile(
        &room(),
        &[
            water(&[([600.0, 64.0, 0.0], [700.0, 192.0, 32.0])]),
            reflection_volume("", [590.0, 54.0, 0.0], [710.0, 202.0, 96.0]),
        ],
    );
    assert!(fixture.place().unwrap().is_empty());
    capture.assert_logged_once(Level::Warn, "entity 2 'reflection_volume' capture point");
    capture.assert_logged(Level::Warn, "is outside the hull; dropped");
}

#[test]
fn capture_offset_that_leaves_the_box_is_a_build_error_naming_the_entity() {
    let fixture = PlacementFixture::compile(
        &room(),
        &[
            water(&[([64.0, 64.0, 0.0], [192.0, 192.0, 32.0])]),
            reflection_volume(
                "\"_capture_offset\" \"200 0\"",
                [48.0, 48.0, 0.0],
                [216.0, 208.0, 96.0],
            ),
        ],
    );
    let error = fixture.place().expect_err("the offset leaves the box");
    let message = format!("{error:#}");
    assert!(
        message.contains("entity 2 'reflection_volume'") && message.contains("outside its box"),
        "{message}"
    );
}

// P13: the smaller box drops for its capture point; the enclosing box takes
// its faces with its own point unchanged, whatever the entity order.
#[test]
fn faces_of_a_dropped_box_bind_the_enclosing_box_or_derived_probes() {
    let pillar = box_brush([200.0, 120.0, 0.0], [208.0, 136.0, 192.0]);
    let pools = water(&[
        ([64.0, 64.0, 0.0], [192.0, 192.0, 32.0]),
        ([320.0, 64.0, 0.0], [448.0, 192.0, 32.0]),
    ]);
    // The offset carries the small box's point into the pillar, inside the box.
    let small = reflection_volume(
        "\"_capture_offset\" \"76 0\"",
        [48.0, 48.0, 0.0],
        [216.0, 208.0, 96.0],
    );
    let large = reflection_volume("", ROOM_BOX.0, ROOM_BOX.1);
    let world = room_with(&[pillar]);

    let capture = LogCapture::start();
    let alone = PlacementFixture::compile(&world, &[pools.clone(), large.clone()])
        .place()
        .unwrap();
    for (entities, large_entity) in [
        (vec![pools.clone(), small.clone(), large.clone()], 3),
        (vec![pools.clone(), large.clone(), small.clone()], 2),
    ] {
        let fixture = PlacementFixture::compile(&world, &entities);
        let faces = fixture.up_faces();
        let probes = fixture.place().unwrap();
        assert_eq!(probes.len(), 1, "{probes:?}");
        assert_eq!(probes[0].source_entity, large_entity);
        assert_eq!(probes[0].capture_point, alone[0].capture_point);
        assert_eq!(
            probes[0].faces.len(),
            faces.len(),
            "every face binds the large box"
        );
    }
    capture.assert_logged(Level::Warn, "capture point");
    capture.assert_logged(Level::Warn, "is in solid; dropped");

    let fixture = PlacementFixture::compile(&world, &[pools, small]);
    let probes = fixture.place().unwrap();
    assert!(!probes.is_empty());
    assert!(
        probes.iter().all(|p| p.source_entity == 1),
        "derived: {probes:?}"
    );
    assert_eq!(
        probes.iter().map(|p| p.faces.len()).sum::<usize>(),
        fixture.up_faces().len()
    );
}

// P27: a box over no water has no capture point.
#[test]
fn authored_box_over_no_water_drops_with_the_binding_no_face_warning_only() {
    let capture = LogCapture::start();
    let fixture = PlacementFixture::compile(
        &room(),
        &[
            water(&[([64.0, 64.0, 0.0], [192.0, 192.0, 32.0])]),
            reflection_volume("", [320.0, 64.0, 0.0], [448.0, 192.0, 96.0]),
        ],
    );
    let probes = fixture.place().unwrap();
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0].source_entity, 1);
    capture.assert_logged_once(
        Level::Warn,
        "entity 2 'reflection_volume' binds no up-facing fluid face; dropped",
    );
    capture.assert_not_logged(Level::Warn, "capture point");
}

#[test]
fn a_pool_spanning_two_authored_boxes_binds_each_face_to_its_own() {
    let fixture = PlacementFixture::compile(
        &room(),
        &[
            water(&[
                ([64.0, 64.0, 0.0], [192.0, 192.0, 32.0]),
                ([192.0, 64.0, 0.0], [320.0, 192.0, 32.0]),
            ]),
            reflection_volume("", [48.0, 48.0, 0.0], [192.0, 208.0, 96.0]),
            reflection_volume("", [192.0, 48.0, 0.0], [336.0, 208.0, 96.0]),
        ],
    );
    let faces = fixture.up_faces();
    let probes = fixture.place().unwrap();
    assert_eq!(
        probes.iter().map(|p| p.source_entity).collect::<Vec<_>>(),
        vec![2, 3]
    );
    let west = face_near(&faces, [128.0, 128.0, 32.0]);
    let east = face_near(&faces, [256.0, 128.0, 32.0]);
    assert_eq!(probes[0].faces, vec![(west.volume, west.face)]);
    assert_eq!(probes[1].faces, vec![(east.volume, east.face)]);
}

// P14 in map units: two 128 × 128 × 96 u boxes 32 u apart both hold the
// pool's centroids. Map units × 0.0254 in f32 give their volumes different
// roundings, yet they tie, so the earlier box in map entity order binds,
// whichever box that is.
#[test]
fn map_unit_boxes_of_one_size_tie_to_the_earlier_entity_in_either_order() {
    let capture = LogCapture::start();
    let pool = water(&[([104.0, 80.0, 0.0], [184.0, 176.0, 32.0])]);
    let west = ([64.0, 64.0, 0.0], [192.0, 192.0, 96.0]);
    let east = ([96.0, 64.0, 0.0], [224.0, 192.0, 96.0]);
    let west_box = reflection_volume("", west.0, west.1);
    let east_box = reflection_volume("", east.0, east.1);
    for (entities, first) in [
        ([pool.clone(), west_box.clone(), east_box.clone()], west),
        ([pool, east_box, west_box], east),
    ] {
        capture.clear();
        let fixture = PlacementFixture::compile(&room(), &entities);
        let probes = fixture.place().unwrap();
        assert_eq!(probes.len(), 1, "{probes:?}");
        assert_eq!(probes[0].source_entity, 2, "the earlier box binds");
        assert_eq!(probes[0].faces.len(), fixture.up_faces().len());
        let (lo, hi) = probe_box(&probes[0]);
        let (expected_lo, expected_hi) = engine_box(first.0, first.1);
        assert_near(lo, expected_lo, 1e-4);
        assert_near(hi, expected_hi, 1e-4);
        capture.assert_logged_once(
            Level::Warn,
            "entity 3 'reflection_volume' binds no up-facing fluid face; dropped",
        );
    }
}

// P21 in map units: two 128 × 128 u pools 192 u apart under a cap of one.
// Their bound areas round differently in f32, yet they tie, so the earlier
// fluid entity keeps its probe, whichever pool that is.
#[test]
fn map_unit_pools_of_one_area_tie_at_the_cap_to_the_earlier_entity_in_either_order() {
    let capture = LogCapture::start();
    let west = water(&[([64.0, 64.0, 0.0], [192.0, 192.0, 32.0])]);
    let east = water(&[([256.0, 64.0, 0.0], [384.0, 192.0, 32.0])]);
    for (entities, first) in [
        ([west.clone(), east.clone()], [128.0, 128.0, 32.0]),
        ([east, west], [320.0, 128.0, 32.0]),
    ] {
        capture.clear();
        let fixture = PlacementFixture::compile(&room(), &entities);
        let faces = fixture.up_faces();
        let probes = fixture.place_with_cap(1).unwrap();
        assert_eq!(probes.len(), 1, "{probes:?}");
        assert_eq!(probes[0].source_entity, 1, "the earlier fluid entity stays");
        let kept = face_near(&faces, first);
        assert!(
            probes[0].faces.contains(&(kept.volume, kept.face)),
            "{probes:?}"
        );
        capture.assert_logged_once(
            Level::Warn,
            "1 derived probe(s) past the 1-probe cap dropped",
        );
    }
}

// ---------------------------------------------------------------- synthetic

/// Placement over a synthetic room with no BSP: no point is in solid or
/// outside the hull, so binding and the cap are all that act.
fn place_synthetic(
    section: &EnvironmentVolumesSection,
    authored: &[MapReflectionVolume],
    cap: usize,
) -> anyhow::Result<Vec<PlacedProbe>> {
    let room = box_room([-8.0, 0.0, -8.0], [8.0, 4.0, 8.0]);
    let (bvh, primitives, _) = build_bvh(&room).unwrap();
    let tracer = SceneTracer::new(&bvh, &primitives, &room);
    let bounds =
        WorldBounds::from_positions(room.geometry.vertices.iter().map(|v| &v.position)).unwrap();
    let tree = BspTree {
        nodes: Vec::new(),
        leaves: Vec::new(),
    };
    let exterior = HashSet::new();
    let classifier = PointClassifier::new(&tree, &exterior, section);
    let control = BakeControl::unrestricted();
    place_probes(
        section,
        authored,
        &PlacementScene {
            tracer: &tracer,
            bounds: &bounds,
            classifier: &classifier,
            governor: control.governor(),
        },
        cap,
    )
}

fn authored_box(entity: u32, min: [f32; 3], max: [f32; 3]) -> MapReflectionVolume {
    MapReflectionVolume {
        source_entity: entity,
        source_label: format!("entity {entity} 'reflection_volume'"),
        min,
        max,
        capture_offset: None,
    }
}

#[test]
fn side_bottom_and_sloped_fluid_faces_bind_no_probe() {
    let pool = vec![
        // Top (face 0): up-facing.
        up_rect(-1.0, -1.0, 1.0, 1.0, 1.0),
        // Bottom (face 1): wound to face down.
        vec![
            [-1.0, 0.5, -1.0],
            [1.0, 0.5, -1.0],
            [1.0, 0.5, 1.0],
            [-1.0, 0.5, 1.0],
        ],
        // Side (face 2).
        vec![
            [1.0, 0.5, -1.0],
            [1.0, 1.0, -1.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.5, 1.0],
        ],
        // Sloped (face 3): 10° off up.
        vec![
            [2.0, 1.0, -1.0],
            [2.0, 1.0, 1.0],
            [3.0, 1.176, 1.0],
            [3.0, 1.176, -1.0],
        ],
    ];
    let section = volumes(vec![fluid_volume(7, &pool)]);
    let probes = place_synthetic(&section, &[], 256).unwrap();
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0].source_entity, 7);
    assert_eq!(probes[0].faces, vec![(0, 0)]);
    // An authored box over every face binds the top alone too.
    let probes = place_synthetic(
        &section,
        &[authored_box(9, [-4.0, 0.0, -4.0], [4.0, 3.0, 4.0])],
        256,
    )
    .unwrap();
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0].source_entity, 9);
    assert_eq!(probes[0].faces, vec![(0, 0)]);
}

// P29 with P32: a centroid off its water moves to the nearest water, which
// can lie past the box's wall; the point is clamped into the box before the
// offset applies, so an offset of "0 0" never fails the build, and an offset
// that carries the clamped point out still does.
#[test]
fn a_zero_capture_offset_holds_when_the_nearest_point_move_leaves_the_box() {
    // A U open toward +z: its area centroid (0, −0.625) lies in the notch,
    // and the nearest water, the inner edge of its base at z = −2, lies
    // south of the box's wall at z = −1.5.
    let u = vec![
        [-3.0, 0.5, -3.0],
        [-3.0, 0.5, 3.0],
        [-2.0, 0.5, 3.0],
        [-2.0, 0.5, -2.0],
        [2.0, 0.5, -2.0],
        [2.0, 0.5, 3.0],
        [3.0, 0.5, 3.0],
        [3.0, 0.5, -3.0],
    ];
    let section = volumes(vec![fluid_volume(1, &[u])]);
    let mut volume = authored_box(4, [-4.0, 0.0, -1.5], [4.0, 3.0, 3.5]);
    volume.capture_offset = Some([0.0, 0.0]);
    let probes = place_synthetic(&section, std::slice::from_ref(&volume), 256).unwrap();
    assert_eq!(probes.len(), 1, "{probes:?}");
    assert_eq!(probes[0].source_entity, 4);
    assert_near(
        point(&probes[0]),
        DVec3::new(0.0, 0.5 + CAPTURE_LIFT_M, -1.5),
        1e-6,
    );

    volume.capture_offset = Some([0.0, -0.1]);
    let error = place_synthetic(&section, &[volume], 256).expect_err("the offset leaves the box");
    let message = format!("{error:#}");
    assert!(
        message.contains("entity 4 'reflection_volume' `_capture_offset` moves its capture point"),
        "{message}"
    );
}

// P14: equal-volume boxes that both hold a centroid, overlapping or sharing
// the face it lies on: the earlier box in map entity order binds.
#[test]
fn equal_smallest_boxes_tie_to_the_earlier_entity() {
    let capture = LogCapture::start();
    // Corners at ±1.5 keep the area centroid exactly at the origin.
    let section = volumes(vec![fluid_volume(1, &[up_rect(-1.5, -1.5, 1.5, 1.5, 0.5)])]);
    for (first, second) in [
        // Overlapping, both 4 × 2 × 4.
        (
            ([-2.0, 0.0, -2.0], [2.0, 2.0, 2.0]),
            ([-1.5, 0.0, -1.5], [2.5, 2.0, 2.5]),
        ),
        // Sharing the face x = 0 the centroid lies on.
        (
            ([-2.0, 0.0, -2.0], [0.0, 2.0, 2.0]),
            ([0.0, 0.0, -2.0], [2.0, 2.0, 2.0]),
        ),
    ] {
        let authored = [
            authored_box(4, first.0, first.1),
            authored_box(5, second.0, second.1),
        ];
        let once = place_synthetic(&section, &authored, 256).unwrap();
        assert_eq!(once.len(), 1);
        assert_eq!(once[0].source_entity, 4);
        assert_eq!(once[0].faces, vec![(0, 0)]);
        assert_eq!(once, place_synthetic(&section, &authored, 256).unwrap());
    }
    capture.assert_logged(
        Level::Warn,
        "entity 5 'reflection_volume' binds no up-facing fluid face; dropped",
    );
}

/// Separate fluid entities, one pool each, of the given areas (m², square),
/// spaced along x so no two share a probe.
fn pools_of_area(areas: &[f32]) -> EnvironmentVolumesSection {
    volumes(
        areas
            .iter()
            .enumerate()
            .map(|(i, &area)| {
                let half = area.sqrt() * 0.5;
                let x = -6.0 + 3.0 * i as f32;
                fluid_volume(
                    10 + i as u32,
                    &[up_rect(x - half, -half, x + half, half, 0.5)],
                )
            })
            .collect(),
    )
}

// P16 with a cap of 3: authored A = 1 and derived D = 4 keep the 2 largest
// derived with one warning naming the count; then A = 4 refuses.
#[test]
fn cap_overflow_drops_the_smallest_derived_and_refuses_authored_past_it() {
    let capture = LogCapture::start();
    let mut section = pools_of_area(&[1.0, 4.0, 2.0, 3.0]);
    section
        .volumes
        .push(fluid_volume(30, &[up_rect(-1.0, 5.0, 1.0, 7.0, 0.5)]));
    let authored = [authored_box(40, [-2.0, 0.0, 4.0], [2.0, 2.0, 7.5])];
    let probes = place_synthetic(&section, &authored, 3).unwrap();
    assert_eq!(
        probes.iter().map(|p| p.source_entity).collect::<Vec<_>>(),
        vec![40, 11, 13],
        "authored first, then the largest derived in fluid entity order"
    );
    capture.assert_logged_once(
        Level::Warn,
        "2 derived probe(s) past the 3-probe cap dropped",
    );
    let bound: Vec<u32> = bindings(&probes).iter().map(|b| b.volume).collect();
    assert_eq!(bound, vec![1, 3, 4], "dropped probes' faces bind nothing");

    let authored: Vec<MapReflectionVolume> = (0..4)
        .map(|i| {
            let x = -6.0 + 3.0 * i as f32;
            authored_box(40 + i, [x - 1.4, 0.0, -1.4], [x + 1.4, 2.0, 1.4])
        })
        .collect();
    let error = place_synthetic(&pools_of_area(&[1.0; 4]), &authored, 3)
        .expect_err("four authored probes past a cap of three");
    assert!(format!("{error:#}").contains("3-probe cap"), "{error:#}");
}

// P26: authored boxes that drop never count toward the cap.
#[test]
fn dropped_authored_boxes_take_no_room_under_the_cap() {
    let section = pools_of_area(&[1.0, 1.0]);
    let authored = [
        authored_box(40, [-7.0, 0.0, -1.0], [-5.0, 2.0, 1.0]),
        authored_box(41, [6.0, 0.0, 6.0], [7.0, 2.0, 7.0]),
        authored_box(42, [-4.0, 0.0, -1.0], [-2.0, 2.0, 1.0]),
        authored_box(43, [6.0, 0.0, -7.0], [7.0, 2.0, -6.0]),
    ];
    let probes = place_synthetic(&section, &authored, 2).unwrap();
    assert_eq!(
        probes.iter().map(|p| p.source_entity).collect::<Vec<_>>(),
        vec![40, 42]
    );
}

// P21: equal bound areas straddling the cap keep the earlier fluid entity,
// then the earlier face; byte-identical across runs and worker counts.
#[test]
fn cap_ties_keep_the_earlier_fluid_entity_then_face() {
    // Whole-metre corners keep the areas exactly equal.
    let section = pools_of_area(&[4.0, 4.0, 4.0]);
    let probes = place_synthetic(&section, &[], 2).unwrap();
    assert_eq!(
        probes.iter().map(|p| p.source_entity).collect::<Vec<_>>(),
        vec![10, 11]
    );

    // One entity, two planes of equal area: the earlier face's probe stays.
    let section = volumes(vec![fluid_volume(
        10,
        &[
            up_rect(2.0, -1.0, 4.0, 1.0, 1.0),
            up_rect(-4.0, -1.0, -2.0, 1.0, 0.5),
        ],
    )]);
    let run = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| place_synthetic(&section, &[], 1).unwrap())
    };
    let one = run(1);
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].faces, vec![(0, 0)]);
    assert_eq!(one, run(4));
    assert_eq!(one, run(1));
}
