//! Emissive volume membership over a real partition: fragment centroid in and
//! out, flush planes, a side split across volumes, atlas sub-faces, faces
//! with no triangle, overlaps, ties, nesting and emptied volumes.

use std::collections::HashSet;

use glam::DVec3;
use log::Level;
use postretro_level_format::emissive::EmissiveVolumeRecord;
use postretro_level_format::geometry::GeometrySection;
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::geometry::{FaceIndexRange, extract_geometry_with_brushes, remap_per_face};
use crate::map_data::MapData;
use crate::map_format::MapFormat;
use crate::pipeline::runtime_cells::{OccluderPartition, RuntimeCells, ShippedCells};

/// Quake map units to engine meters.
const UNIT: f64 = 0.0254;
const GLOW: &str = "glow";

fn box_brush(min: [i32; 3], max: [i32; 3], texture: &str) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        r#"{{
( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {texture} 0 0 0 1 1
( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {texture} 0 0 0 1 1
( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {texture} 0 0 0 1 1
( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {texture} 0 0 0 1 1
( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {texture} 0 0 0 1 1
( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {texture} 0 0 0 1 1
}}"#
    )
}

/// An emissive volume over the map-unit box `min..max`.
fn volume(min: [i32; 3], max: [i32; 3]) -> String {
    format!(
        "{{\n\"classname\" \"emissive_volume\"\n{}\n}}",
        box_brush(min, max, "trigger")
    )
}

/// A sealed 256² × 128 room (interior map units −128..128, 0..128) with two
/// glowing panels on the floor: panel 1 at −16..16, panel 2 at 64..80, each
/// 8 high. `extra` entities follow worldspawn (entity 0) in order.
fn room(extra: &[String]) -> String {
    let brushes = [
        box_brush([-144, -144, -16], [144, 144, 0], "floor"),
        box_brush([-144, -144, 128], [144, 144, 144], "wall"),
        box_brush([-144, -144, 0], [-128, 144, 128], "wall"),
        box_brush([128, -144, 0], [144, 144, 128], "wall"),
        box_brush([-128, -144, 0], [128, -128, 128], "wall"),
        box_brush([-128, 128, 0], [128, 144, 128], "wall"),
        box_brush([-16, -16, 0], [16, 16, 8], GLOW),
        box_brush([64, 64, 0], [80, 80, 8], GLOW),
    ];
    let mut entities = vec![format!(
        "{{\n\"classname\" \"worldspawn\"\n{}\n}}",
        brushes.join("\n")
    )];
    entities.extend(extra.iter().cloned());
    entities.join("\n")
}

struct World {
    map: MapData,
    geometry: GeometryResult,
    face_brushes: Vec<Option<u32>>,
}

impl World {
    fn new(map_text: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "postretro-emissive-membership-{}-{id}.map",
            std::process::id()
        ));
        std::fs::write(&path, format!("{map_text}\n")).expect("write fixture map");
        let map = crate::parse::parse_map_file(&path, MapFormat::IdTech2);
        let _ = std::fs::remove_file(&path);
        let map = map.unwrap_or_else(|error| panic!("parse fixture map: {error:#}"));
        let ShippedCells {
            partition: OccluderPartition { result, .. },
            runtime: RuntimeCells { cells, .. },
            ..
        } = ShippedCells::build(&map.brush_volumes, &map.kinematic_movers).expect("partition");
        let (geometry, face_brushes) =
            extract_geometry_with_brushes(&result.faces, &cells.tree, &cells.exterior_cells);
        Self {
            map,
            geometry,
            face_brushes,
        }
    }

    fn has_emissive_map(&self) -> Vec<bool> {
        self.geometry
            .texture_names
            .names
            .iter()
            .map(|name| name == GLOW)
            .collect()
    }

    /// Records over the uncut world, each face at its own fragment's centroid.
    fn records(&self) -> Vec<EmissiveVolumeRecord> {
        resolve_emissive_volume_records(
            &self.map.emissive_volumes,
            &self.geometry,
            &fragment_centroids(&self.geometry),
            &self.has_emissive_map(),
        )
    }

    /// Faces of `texture` whose every vertex lies in the map-unit box.
    fn faces_within(&self, texture: &str, min: [f64; 3], max: [f64; 3]) -> Vec<u32> {
        faces_within(&self.geometry, texture, min, max)
    }
}

fn faces_within(
    geometry: &GeometryResult,
    texture: &str,
    min: [f64; 3],
    max: [f64; 3],
) -> Vec<u32> {
    let (lo, hi) = (engine(min), engine(max));
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    let slack = DVec3::splat(1e-4);
    (0..geometry.geometry.faces.len())
        .filter(|&face| {
            let meta = &geometry.geometry.faces[face];
            geometry.texture_names.names[meta.texture_index as usize] == texture
                && face_indices(face, geometry).unwrap().iter().all(|&vertex| {
                    let p = DVec3::from(
                        geometry.geometry.vertices[vertex as usize]
                            .position
                            .map(f64::from),
                    );
                    p.cmpge(lo - slack).all() && p.cmple(hi + slack).all()
                })
        })
        .map(|face| face as u32)
        .collect()
}

fn engine(q: [f64; 3]) -> DVec3 {
    crate::parse::quake_to_engine(DVec3::from(q)) * UNIT
}

fn panel_one(world: &World) -> Vec<u32> {
    let faces = world.faces_within(GLOW, [-16.0, -16.0, 0.0], [16.0, 16.0, 8.0]);
    assert!(faces.len() >= 5, "a top and four sides: {faces:?}");
    faces
}

fn panel_one_top(world: &World) -> Vec<u32> {
    let faces = world.faces_within(GLOW, [-16.0, -16.0, 8.0], [16.0, 16.0, 8.0]);
    assert!(!faces.is_empty());
    faces
}

fn panel_two(world: &World) -> Vec<u32> {
    let faces = world.faces_within(GLOW, [64.0, 64.0, 0.0], [80.0, 80.0, 8.0]);
    assert!(faces.len() >= 5, "{faces:?}");
    faces
}

// Acceptance A5: a face whose fragment's centroid is inside a hull joins;
// one just outside does not.
#[test]
fn a_face_joins_when_its_centroid_is_inside_and_not_just_outside() {
    let world = World::new(&room(&[volume([-32, -32, 1], [32, 32, 32])]));
    let capture = LogCapture::start();
    let records = world.records();
    assert_eq!(
        records[0].faces,
        panel_one(&world),
        "the panel's faces, no floor or wall"
    );
    capture.assert_nothing_above(Level::Info);

    // One map unit above the panel's top: nothing inside.
    let world = World::new(&room(&[volume([-32, -32, 9], [32, 32, 32])]));
    assert!(world.records()[0].faces.is_empty());
}

// Pin P27: a face in the plane of a volume brush's side joins.
#[test]
fn a_face_in_the_plane_of_a_volume_side_joins() {
    let world = World::new(&room(&[volume([-32, -32, 8], [32, 32, 32])]));
    assert_eq!(world.records()[0].faces, panel_one_top(&world));

    // A volume whose bottom is flush on the floor holds every floor fragment;
    // raised one map unit, none.
    let world = World::new(&room(&[volume([-128, -128, 0], [128, 128, 4])]));
    let faces = &world.records()[0].faces;
    let floor = world.faces_within("floor", [-128.0, -128.0, 0.0], [128.0, 128.0, 0.0]);
    assert!(!floor.is_empty());
    assert!(floor.iter().all(|face| faces.contains(face)), "{faces:?}");
    let world = World::new(&room(&[volume([-128, -128, 1], [128, 128, 4])]));
    let faces = &world.records()[0].faces;
    let floor = world.faces_within("floor", [-128.0, -128.0, 0.0], [128.0, 128.0, 0.0]);
    assert!(!floor.iter().any(|face| faces.contains(face)), "{faces:?}");
}

/// Two sealed rooms split by a wall at map x −8..8, sharing one glowing
/// floor brush whose top side the BSP splits into a fragment set per room.
/// `extra` entities follow worldspawn.
fn divided_room(extra: &[String]) -> String {
    let brushes = [
        box_brush([-144, -144, -16], [144, 144, 0], GLOW),
        box_brush([-144, -144, 128], [144, 144, 144], "wall"),
        box_brush([-144, -144, 0], [-128, 144, 128], "wall"),
        box_brush([128, -144, 0], [144, 144, 128], "wall"),
        box_brush([-128, -144, 0], [128, -128, 128], "wall"),
        box_brush([-128, 128, 0], [128, 144, 128], "wall"),
        box_brush([-8, -128, 0], [8, 128, 128], "wall"),
    ];
    let mut entities = vec![format!(
        "{{\n\"classname\" \"worldspawn\"\n{}\n}}",
        brushes.join("\n")
    )];
    entities.extend(extra.iter().cloned());
    entities.join("\n")
}

// Owner ruling (membership per final face): one glowing brush side spanning
// two volumes splits between them by BSP fragment, silently.
#[test]
fn one_brush_side_spanning_two_volumes_splits_by_fragment() {
    let world = World::new(&divided_room(&[
        volume([-128, -128, 0], [-8, 128, 32]),
        volume([8, -128, 0], [128, 128, 32]),
    ]));
    let floor = world.faces_within(GLOW, [-128.0, -128.0, 0.0], [128.0, 128.0, 0.0]);
    let west = world.faces_within(GLOW, [-128.0, -128.0, 0.0], [-8.0, 128.0, 0.0]);
    let east = world.faces_within(GLOW, [8.0, -128.0, 0.0], [128.0, 128.0, 0.0]);
    assert!(!west.is_empty() && !east.is_empty(), "{west:?} {east:?}");
    assert_eq!(
        west.len() + east.len(),
        floor.len(),
        "every floor fragment lies in one room: {floor:?}"
    );
    let brushes: HashSet<Option<u32>> = floor
        .iter()
        .map(|&face| world.face_brushes[face as usize])
        .collect();
    assert_eq!(brushes.len(), 1, "one authored brush side: {brushes:?}");

    let capture = LogCapture::start();
    let records = world.records();
    assert_eq!(records[0].faces, west);
    assert_eq!(records[1].faces, east);
    capture.assert_nothing_above(Level::Info);
}

/// `geometry` with face `target` cut into one sub-face per triangle, each
/// owning its vertices, and the cut's `face_remap`, as the atlas stage
/// returns it.
fn cut_into_triangles(
    geometry: &GeometryResult,
    target: usize,
) -> (GeometryResult, Vec<std::ops::Range<usize>>) {
    let mut cut = GeometryResult {
        geometry: GeometrySection {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
        },
        texture_names: geometry.texture_names.clone(),
        face_index_ranges: Vec::new(),
    };
    let mut remap = Vec::new();
    for face in 0..geometry.geometry.faces.len() {
        let indices = face_indices(face, geometry).unwrap();
        let pieces: Vec<&[u32]> = if face == target {
            indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| t.as_slice())
                .collect()
        } else {
            vec![indices]
        };
        let start = cut.geometry.faces.len();
        for piece in pieces {
            let index_offset = cut.geometry.indices.len() as u32;
            for &vertex in piece {
                cut.geometry
                    .indices
                    .push(cut.geometry.vertices.len() as u32);
                cut.geometry
                    .vertices
                    .push(geometry.geometry.vertices[vertex as usize].clone());
            }
            cut.geometry
                .faces
                .push(geometry.geometry.faces[face].clone());
            cut.face_index_ranges.push(FaceIndexRange {
                index_offset,
                index_count: piece.len() as u32,
            });
        }
        remap.push(start..cut.geometry.faces.len());
    }
    (cut, remap)
}

// Pin P29: every sub-face the atlas cuts from a member fragment carries its
// membership through the cut's remap, though a sub-face's own centroid lies
// outside the volume.
#[test]
fn atlas_sub_faces_share_their_fragment_membership() {
    // A 4-unit box around the panel top's centre.
    let world = World::new(&room(&[volume([-2, -2, 4], [2, 2, 12])]));
    let top = panel_one_top(&world);
    assert_eq!(top.len(), 1, "the panel top is one fragment: {top:?}");
    let before_cut = fragment_centroids(&world.geometry);
    let (cut, remap) = cut_into_triangles(&world.geometry, top[0] as usize);
    let pieces = faces_within(&cut, GLOW, [-16.0, -16.0, 8.0], [16.0, 16.0, 8.0]);
    assert!(pieces.len() >= 2, "the top is cut in two: {pieces:?}");
    let hull = &world.map.emissive_volumes[0].hulls[0];
    for &piece in &pieces {
        let own = face_centroid(piece as usize, &cut).unwrap();
        assert!(
            hull.planes.iter().any(|&(n, d)| n.dot(own) - d > 0.01),
            "sub-face {piece}'s own centroid lies outside the volume"
        );
    }

    let records = resolve_emissive_volume_records(
        &world.map.emissive_volumes,
        &cut,
        &remap_per_face(&before_cut, &remap),
        &world.has_emissive_map(),
    );
    assert_eq!(
        records[0].faces, pieces,
        "every piece joins, and nothing else"
    );
    // Each piece at its own centroid would join nothing: the remap is what
    // keeps the fragment whole.
    let own = resolve_emissive_volume_records(
        &world.map.emissive_volumes,
        &cut,
        &fragment_centroids(&cut),
        &world.has_emissive_map(),
    );
    assert!(own[0].faces.is_empty(), "{:?}", own[0].faces);
}

// A face with no triangle joins no volume, though the volume holds the
// origin a centroid of no triangles would fall back to.
#[test]
fn a_face_with_no_triangle_joins_nothing() {
    let world = World::new(&room(&[volume([-32, -32, -1], [32, 32, 32])]));
    let panel = panel_one(&world);
    let emptied = panel_one_top(&world)[0];
    let points = fragment_centroids(&world.geometry);
    let mut geometry = world.geometry.clone();
    geometry.face_index_ranges[emptied as usize].index_count = 0;
    assert_eq!(fragment_centroids(&geometry)[emptied as usize], None);
    // Whether its point was taken before the triangles went (a sub-face
    // emptied after the cut) or after, the face stays out.
    for points in [points, fragment_centroids(&geometry)] {
        let records = resolve_emissive_volume_records(
            &world.map.emissive_volumes,
            &geometry,
            &points,
            &world.has_emissive_map(),
        );
        let faces = &records[0].faces;
        assert!(!faces.contains(&emptied), "{faces:?}");
        assert!(
            panel
                .iter()
                .filter(|&&face| face != emptied)
                .all(|face| faces.contains(face)),
            "the rest of the panel still joins: {faces:?}"
        );
    }
}

// Once any volume holds a face, the loader reaches every face through its BVH
// leaf, so a triangle-less face anywhere, even far from every volume, fails
// the build right after membership, naming a volume, the face, its texture
// and brush, and the remedy. With no volume holding a face the build goes on.
#[test]
fn members_beside_a_triangle_less_face_fail_naming_volume_face_and_brush() {
    let world = World::new(&room(&[
        volume([-32, -32, -1], [32, 32, 32]),
        volume([56, 56, -1], [88, 88, 32]),
    ]));
    let floor = world.faces_within("floor", [-144.0, -144.0, -16.0], [144.0, 144.0, 0.0]);
    let emptied = floor[0] as usize;
    let mut geometry = world.geometry.clone();
    geometry.face_index_ranges[emptied].index_count = 0;
    let records = resolve_emissive_volume_records(
        &world.map.emissive_volumes,
        &geometry,
        &fragment_centroids(&geometry),
        &world.has_emissive_map(),
    );
    assert!(records.iter().all(|record| !record.faces.is_empty()));
    let brush = world.face_brushes[emptied].expect("a floor face has its brush");
    let message = refuse_members_beside_a_triangle_less_face(
        &world.map.emissive_volumes,
        &records,
        &geometry,
        &world.face_brushes,
    )
    .expect_err("members beside a triangle-less face are refused")
    .to_string();
    let named_face =
        format!("world face {emptied} (texture 'floor', brush {brush}) has no triangle");
    for expected in [
        "entity 1 'emissive_volume'",
        "(and 1 more emissive volume(s)) holds faces",
        named_face.as_str(),
        "Remove or rebuild that degenerate brush, or remove the emissive volumes",
    ] {
        assert!(message.contains(expected), "{expected:?} in {message}");
    }

    // A volume in empty air holds nothing: the loader never looks.
    let world = World::new(&room(&[volume([-100, -100, 60], [-90, -90, 70])]));
    let floor = world.faces_within("floor", [-144.0, -144.0, -16.0], [144.0, 144.0, 0.0]);
    let mut geometry = world.geometry.clone();
    geometry.face_index_ranges[floor[0] as usize].index_count = 0;
    let records = resolve_emissive_volume_records(
        &world.map.emissive_volumes,
        &geometry,
        &fragment_centroids(&geometry),
        &world.has_emissive_map(),
    );
    assert!(records[0].faces.is_empty());
    refuse_members_beside_a_triangle_less_face(
        &world.map.emissive_volumes,
        &records,
        &geometry,
        &world.face_brushes,
    )
    .expect("no volume holds a face");
}

// Acceptance A5: of two volumes containing a face, the smaller wins, in
// either map order; the larger, left with no glowing face, warns (P28).
#[test]
fn the_smaller_containing_volume_wins_in_either_order() {
    let small = volume([-32, -32, 1], [32, 32, 32]);
    let large = volume([-64, -64, 1], [64, 64, 32]);
    for (volumes, small_index, large_entity) in [
        ([small.clone(), large.clone()], 0, 2),
        ([large.clone(), small.clone()], 1, 1),
    ] {
        let world = World::new(&room(&volumes));
        let capture = LogCapture::start();
        let records = world.records();
        let panel = panel_one(&world);
        assert_eq!(records[small_index].faces, panel);
        let emptied = &records[1 - small_index];
        assert!(emptied.faces.is_empty());
        // Pin P28: the emptied volume keeps its record and origin.
        let origin = engine([0.0, 0.0, 16.5]).as_vec3().to_array();
        for (actual, expected) in emptied.origin.iter().zip(origin) {
            assert!((actual - expected).abs() < 1e-5, "{:?}", emptied.origin);
        }
        assert_eq!(
            (emptied.min, emptied.max, emptied.color),
            (0.0, 1.0, [1.0; 3])
        );
        capture.assert_logged_once(
            Level::Warn,
            &format!("entity {large_entity} 'emissive_volume' at ("),
        );
        capture.assert_logged_once(
            Level::Warn,
            &format!(
                "holds no face with an `_e` map: smaller or later emissive volumes take all {} \
                 glowing face(s) inside it",
                panel.len()
            ),
        );
        assert_eq!(
            capture
                .records()
                .iter()
                .filter(|record| record.level <= Level::Warn)
                .count(),
            1
        );
    }
}

// Acceptance A5: an exact tie goes to the later volume in map order.
#[test]
fn an_exact_tie_goes_to_the_later_volume() {
    let world = World::new(&room(&[
        volume([-32, -32, 1], [32, 32, 32]),
        volume([-32, -32, 1], [32, 32, 32]),
    ]));
    let records = world.records();
    assert!(records[0].faces.is_empty(), "the earlier loses the tie");
    assert_eq!(records[1].faces, panel_one(&world));
}

// Pin P57: a volume nested in another keeps its faces whichever comes first,
// and the outer keeps the rest, silently.
#[test]
fn a_nested_volume_keeps_its_faces_in_either_order_silently() {
    let inner = volume([-32, -32, 1], [32, 32, 32]);
    let outer = volume([-100, -100, 1], [100, 100, 32]);
    for (volumes, inner_index) in [([inner.clone(), outer.clone()], 0), ([outer, inner], 1)] {
        let world = World::new(&room(&volumes));
        let capture = LogCapture::start();
        let records = world.records();
        assert_eq!(records[inner_index].faces, panel_one(&world));
        assert_eq!(records[1 - inner_index].faces, panel_two(&world));
        capture.assert_nothing_above(Level::Info);
    }
}

// Pin P28: a volume around faces none of which has an `_e` map warns,
// naming it by entity, tags and origin.
#[test]
fn a_volume_with_no_glowing_face_warns_naming_it() {
    // In the air above the floor: it holds no face at all.
    let world = World::new(&room(&[format!(
        "{{\n\"classname\" \"emissive_volume\"\n\"_tags\" \"lobby sign\"\n{}\n}}",
        box_brush([-120, -120, 1], [-100, -100, 3], "trigger")
    )]));
    let capture = LogCapture::start();
    let records = world.records();
    assert!(records[0].faces.is_empty(), "{:?}", records[0].faces);
    capture.assert_logged_once(
        Level::Warn,
        "entity 1 'emissive_volume' (tags: lobby sign) at (2.794, 0.051, 2.794) m holds no face \
         with an `_e` map; it glows nothing",
    );
}

// Acceptance A5: a volume enclosing a mover brush and a fluid lists neither;
// both bind only their texture's strength.
#[test]
fn a_volume_enclosing_a_mover_and_a_fluid_lists_neither() {
    let mover = format!(
        "{{\n\"classname\" \"kinematic_mover\"\n\"name\" \"lift\"\n\"path\" \"wp_a\"\n\
         \"speed\" \"2.5\"\n\"wait_ms\" \"50\"\n\"move_mode\" \"ping_pong\"\n\
         \"start_on_spawn\" \"1\"\n{}\n}}",
        box_brush([-16, -16, 40], [16, 16, 56], GLOW)
    );
    let fluid = format!(
        "{{\n\"classname\" \"fluid_volume\"\n\"fluid\" \"water\"\n{}\n}}",
        box_brush([24, -16, 40], [40, 16, 56], GLOW)
    );
    let waypoints = [
        "{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"wp_a\"\n\"next\" \"wp_b\"\n\
         \"origin\" \"0 0 48\"\n}"
            .to_string(),
        "{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"wp_b\"\n\"origin\" \"0 0 96\"\n}"
            .to_string(),
    ];
    let world = World::new(&room(&[
        mover,
        fluid,
        waypoints[0].clone(),
        waypoints[1].clone(),
        volume([-48, -48, 32], [48, 48, 64]),
    ]));
    assert_eq!(world.map.kinematic_movers.len(), 1);
    assert_eq!(world.map.environment_volumes.len(), 1);
    let records = world.records();
    assert!(records[0].faces.is_empty(), "{:?}", records[0].faces);
}

// Every face sits in at most one volume, each list ascending: id 60's
// ownership contract.
#[test]
fn face_lists_ascend_and_each_face_is_owned_once() {
    let world = World::new(&room(&[
        volume([-100, -100, 0], [100, 100, 32]),
        volume([-32, -32, 0], [32, 32, 32]),
        volume([-140, -140, 0], [140, 140, 140]),
    ]));
    let records = world.records();
    let mut owned = HashSet::new();
    for record in &records {
        assert!(record.faces.windows(2).all(|pair| pair[0] < pair[1]));
        for &face in &record.faces {
            assert!(owned.insert(face), "face {face} owned twice");
        }
    }
    assert!(!records.iter().all(|record| record.faces.is_empty()));
}

#[test]
fn a_map_without_volumes_writes_no_records_and_logs_nothing() {
    let world = World::new(&room(&[]));
    let capture = LogCapture::start();
    assert!(world.records().is_empty());
    capture.assert_nothing_above(Level::Debug);
}
