//! Seam closure: every fixture leaves a fluid seam open before closure, and
//! after it every face that meets another carries its vertices bit for bit,
//! within a volume and across volumes. Nothing else moves: face count, order,
//! texture and side projection hold, corners move at most the tolerance, and
//! no face degenerates. Sub-millimetre cases are built as exact polygons.

use super::*;
use crate::environment_volumes::closure::{
    ClosureStats, centroid_and_normal, close_seams, encoded, segment_distance,
};
use crate::environment_volumes::surface::SurfacePiece;
use crate::map_data::{BrushSide, TextureProjection};
use crate::reflection_probes::placement::FaceGeometry;

/// Millimetres to meters.
const MILLI: f64 = 1e-3;

/// Closure's tolerance, meters.
const TOLERANCE: f64 = SEAM_TOLERANCE;

/// Asserts the faces before closure leave a seam open, closure closes every
/// one and moves nothing else; returns the closed section.
#[track_caller]
fn assert_closes(world: &[String], volumes: &[String]) -> EnvironmentVolumesSection {
    let before = compile_unclosed(world, volumes);
    assert!(
        !open_seams(&before).is_empty(),
        "the fixture leaves no seam open before closure"
    );
    let (_, after) = compile(world, volumes);
    assert_eq!(open_seams(&after), Vec::new(), "closure leaves seams open");
    assert_nothing_else_moves(&before, &after, TOLERANCE);
    after
}

/// The texel UV the face's own projection gives `position`, fitted from the
/// face before closure: UVs are affine in position on a face's plane.
fn projected_uv(face: &EnvironmentSurfaceFace, position: DVec3) -> [f64; 2] {
    let points = positions(face);
    let uv = |i: usize| face.vertices[i].uv.map(f64::from);
    // The widest triangle from the first vertex, for a well-conditioned fit.
    let (b, c) = (1..points.len())
        .flat_map(|b| (b + 1..points.len()).map(move |c| (b, c)))
        .max_by(|&(b1, c1), &(b2, c2)| {
            let area = |b: usize, c: usize| {
                (points[b] - points[0])
                    .cross(points[c] - points[0])
                    .length()
            };
            area(b1, c1).total_cmp(&area(b2, c2))
        })
        .expect("a face has three vertices");
    let (e1, e2) = (points[b] - points[0], points[c] - points[0]);
    let normal = e1.cross(e2);
    let local = position - points[0];
    // Barycentric weights of `local` against e1 and e2 in the face's plane.
    let w1 = local.cross(e2).dot(normal) / normal.length_squared();
    let w2 = e1.cross(local).dot(normal) / normal.length_squared();
    [0, 1].map(|axis| {
        uv(0)[axis] + w1 * (uv(b)[axis] - uv(0)[axis]) + w2 * (uv(c)[axis] - uv(0)[axis])
    })
}

/// Closure kept every record's face count, order and texture, and every
/// face's UVs on its own side's projection; each face keeps its shape
/// ([`assert_face_kept`]).
#[track_caller]
pub(super) fn assert_nothing_else_moves(
    before: &EnvironmentVolumesSection,
    after: &EnvironmentVolumesSection,
    reach: f64,
) {
    assert_eq!(before.volumes.len(), after.volumes.len());
    for (record, (was, now)) in before.volumes.iter().zip(&after.volumes).enumerate() {
        assert_eq!(
            was.surface_faces.len(),
            now.surface_faces.len(),
            "record {record}"
        );
        for (old, new) in was.surface_faces.iter().zip(&now.surface_faces) {
            assert_eq!(old.texture_index, new.texture_index);
            let new_points = positions(new);
            for (point, vertex) in new_points.iter().zip(&new.vertices) {
                let want = projected_uv(old, *point);
                let got = vertex.uv.map(f64::from);
                assert!(
                    (got[0] - want[0]).abs() < 1e-2 && (got[1] - want[1]).abs() < 1e-2,
                    "UV {got:?} is not its side's projection {want:?} at {point}"
                );
            }
            assert_face_kept(&positions(old), &new_points, reach);
        }
    }
}

/// One face before and after closure, as encoded: every vertex after lies
/// within `reach` of the boundary before; every corner before has a vertex
/// after within the tolerance; every vertex closure added lay within the
/// tolerance of the face's boundary as closure then left it
/// ([`assert_added_vertices_peel`]); the face keeps three distinct vertices,
/// a normal facing the way it did, its area within the tolerance times its
/// perimeter, and a boundary that runs in order round its area centroid.
#[track_caller]
fn assert_face_kept(was: &[DVec3], now: &[DVec3], reach: f64) {
    for point in now {
        assert!(
            boundary_distance(was, *point) <= reach,
            "{point} lies {} m off its face's boundary before closure",
            boundary_distance(was, *point)
        );
    }
    for corner in was {
        assert!(
            now.iter().any(|p| p.distance(*corner) <= TOLERANCE),
            "corner {corner} moved past the tolerance"
        );
    }
    assert_added_vertices_peel(was, now);
    let mut distinct: Vec<[u64; 3]> = now.iter().map(|p| p.to_array().map(f64::to_bits)).collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(distinct.len() >= 3, "a face collapsed: {now:?}");
    let geometry = FaceGeometry::from_polygon(now).expect("a face keeps a normal");
    let old_geometry = FaceGeometry::from_polygon(was).expect("the fixture face has a normal");
    assert!(
        geometry.normal.dot(old_geometry.normal) > 0.0,
        "the face flipped from {} to {}: {now:?}",
        old_geometry.normal,
        geometry.normal
    );
    let perimeter: f64 = (0..was.len())
        .map(|i| was[i].distance(was[(i + 1) % was.len()]))
        .sum();
    assert!(
        (geometry.area - old_geometry.area).abs() <= TOLERANCE * perimeter,
        "area {} moved from {}",
        geometry.area,
        old_geometry.area
    );
    assert_runs_in_order(now);
}

/// The vertices closure added to `now` (those not within the tolerance of a
/// corner of `was`) peel back off: one at a time where it lies within the
/// tolerance of the segment its ring neighbours span, or failing that of the
/// rest of the ring's boundary, or as a run of added vertices within the
/// tolerance of the segment the kept vertices either side span, as inserts
/// judged together against one edge are. So, in some insertion order, each
/// added vertex lay within the tolerance of the face's boundary as closure
/// then left it (P18, P22). The boundary fallback is the edge choice's
/// allowance: a vertex joins the edge whose sector about the area centroid
/// holds it, and beside a near-flat corner that edge may lie about half a
/// millimetre off.
#[track_caller]
fn assert_added_vertices_peel(was: &[DVec3], now: &[DVec3]) {
    let mut ring: Vec<(DVec3, bool)> = now
        .iter()
        .map(|&point| {
            let kept = was.iter().any(|corner| corner.distance(point) <= TOLERANCE);
            (point, !kept)
        })
        .collect();
    while ring.iter().any(|&(_, added)| added) {
        let count = ring.len();
        let on_chord = |i: usize| {
            segment_distance(
                ring[(i + count - 1) % count].0,
                ring[(i + 1) % count].0,
                ring[i].0,
            ) <= TOLERANCE
        };
        let on_rest = |i: usize| {
            let rest: Vec<DVec3> = (0..count).filter(|&j| j != i).map(|j| ring[j].0).collect();
            boundary_distance(&rest, ring[i].0) <= TOLERANCE
        };
        let single = (0..count)
            .find(|&i| ring[i].1 && on_chord(i))
            .or_else(|| (0..count).find(|&i| ring[i].1 && on_rest(i)));
        if let Some(i) = single {
            ring.remove(i);
            continue;
        }
        // Runs of added vertices between kept ones, each judged as a block.
        let start = ring
            .iter()
            .position(|&(_, added)| !added)
            .expect("closure keeps a vertex near each corner");
        ring.rotate_left(start);
        let mut peeled = false;
        let mut i = 0;
        while i < ring.len() {
            let end = (i + 1..ring.len())
                .find(|&j| !ring[j].1)
                .unwrap_or(ring.len());
            let (from, to) = (ring[i].0, ring[end % ring.len()].0);
            if end > i + 1
                && ring[i + 1..end]
                    .iter()
                    .all(|&(point, _)| segment_distance(from, to, point) <= TOLERANCE)
            {
                ring.drain(i + 1..end);
                peeled = true;
                break;
            }
            i = end;
        }
        assert!(
            peeled,
            "added vertices lie off every edge they could have joined: {now:?}"
        );
    }
}

/// The angle about `polygon`'s area centroid strictly increases from each
/// vertex to the next, once round.
#[track_caller]
fn assert_runs_in_order(polygon: &[DVec3]) {
    let (centre, normal) = centroid_and_normal(polygon);
    let mut total = 0.0;
    for (i, &point) in polygon.iter().enumerate() {
        let (a, b) = (point - centre, polygon[(i + 1) % polygon.len()] - centre);
        let turn = a.cross(b).dot(normal).atan2(a.dot(b) * normal.length());
        assert!(
            turn > 0.0,
            "the boundary turns back at {point}: {polygon:?}"
        );
        total += turn;
    }
    assert!(
        (total - std::f64::consts::TAU).abs() < 1e-6,
        "the boundary winds {total} rad round its centroid"
    );
}

// --- Map fixtures --------------------------------------------------------

#[test]
fn a_partly_buried_side_closes_against_the_top() {
    // The wall covers the fluid's -x face only below z = 64.
    let wall = box_brush([-32, -128, -128], [0, 128, 64], "world_tex");
    assert_closes(
        &[wall],
        &[fluid(&[box_brush([0, -32, 32], [64, 32, 96], "water_tex")])],
    );
}

#[test]
fn unequal_siblings_close_along_their_shared_edges() {
    // Two brushes sharing a top plane, the east one twice as wide.
    assert_closes(
        &distant_world(),
        &[fluid(&[
            box_brush([0, 0, 0], [64, 64, 64], "water_tex"),
            box_brush([64, 0, 0], [128, 128, 64], "water_tex"),
        ])],
    );
}

// P1: closure sees the push's split pieces as well as burial's.
#[test]
fn a_buried_volume_a_push_cut_closes_every_seam() {
    let wall = box_brush([-32, -128, -128], [0, 128, 32], "world_tex");
    let current = entity(
        "push_volume",
        "\"push\" \"0 8 0\"",
        &[box_brush([96, 48, 0], [160, 96, 64], "push_tex")],
    );
    assert_closes(
        &[wall],
        &[
            fluid(&[box_brush([0, 0, 0], [128, 64, 64], "water_tex")]),
            current,
        ],
    );
}

// P2: each volume gains the other's vertices whichever comes first.
#[test]
fn two_fluids_side_by_side_close_the_same_in_either_entity_order() {
    let water = fluid(&[box_brush([0, 0, 0], [128, 64, 64], "water_tex")]);
    let lava = entity(
        "fluid_volume",
        "\"fluid\" \"lava\"",
        &[box_brush([128, 0, 0], [192, 32, 64], "lava_tex")],
    );
    let vertex_sets = |section: &EnvironmentVolumesSection| {
        let mut by_fluid: Vec<(String, Vec<[u32; 3]>)> = section
            .volumes
            .iter()
            .map(|record| {
                let mut points: Vec<[u32; 3]> = record
                    .surface_faces
                    .iter()
                    .flat_map(|face| &face.vertices)
                    .map(|vertex| vertex.position.map(f32::to_bits))
                    .collect();
                points.sort_unstable();
                (record.fluid.clone(), points)
            })
            .collect();
        by_fluid.sort();
        by_fluid
    };
    let forward = assert_closes(&distant_world(), &[water.clone(), lava.clone()]);
    let reverse = assert_closes(&distant_world(), &[lava, water]);
    assert_eq!(vertex_sets(&forward), vertex_sets(&reverse));
}

#[test]
fn a_top_meeting_another_volumes_side_closes() {
    assert_closes(
        &distant_world(),
        &[
            fluid(&[box_brush([0, 0, 0], [128, 64, 64], "water_tex")]),
            fluid(&[box_brush([128, 0, 0], [256, 64, 128], "water_tex")]),
        ],
    );
}

/// A box brush whose top slopes from `z_west` at `x0` to `z_east` at `x1`.
fn sloped_brush(x: [i32; 2], y: [i32; 2], z0: i32, top: [i32; 2], texture: &str) -> String {
    let ([x0, x1], [y0, y1], [z_west, z_east]) = (x, y, top);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {texture} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {texture} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {texture} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {texture} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {texture} 0 0 0 1 1\n\
         ( {x0} 0 {z_west} ) ( {x0} 1 {z_west} ) ( {x1} 0 {z_east} ) {texture} 0 0 0 1 1\n\
         }}"
    )
}

#[test]
fn a_sloped_brush_closes_against_its_neighbour() {
    assert_closes(
        &distant_world(),
        &[fluid(&[
            box_brush([0, 0, 0], [64, 64, 64], "water_tex"),
            sloped_brush([64, 128], [0, 128], 0, [64, 32], "water_tex"),
        ])],
    );
}

// P15: faces gather level-wide and return to their own (record, face) slots;
// a record with no faces between fluid records keeps none.
#[test]
fn records_without_faces_between_fluid_volumes_keep_their_slots() {
    let floor = box_brush([-512, -512, -512], [512, 512, -256], "world_tex");
    let gravity = entity(
        "gravity_volume",
        "\"gravity\" \"-3\"",
        &[box_brush([0, 0, 0], [64, 64, 64], "volume_tex")],
    );
    let buried = fluid(&[box_brush([0, 0, -400], [64, 64, -300], "water_tex")]);
    let after = assert_closes(
        &[floor],
        &[
            fluid(&[box_brush([0, 0, 0], [128, 64, 64], "water_tex")]),
            gravity,
            buried,
            fluid(&[box_brush([128, 0, 0], [256, 64, 128], "water_tex")]),
        ],
    );
    assert!(after.volumes[1].surface_faces.is_empty());
    assert!(after.volumes[2].surface_faces.is_empty());
}

// P15: one face, or none, is left exactly as it was.
#[test]
fn a_level_with_one_fluid_face_or_none_is_left_as_it_was() {
    // A pool in a pit carved into the floor draws only its top.
    let pit = vec![
        box_brush([-64, -64, -64], [192, 192, -48], "world_tex"),
        box_brush([-64, -64, -48], [0, 192, 0], "world_tex"),
        box_brush([128, -64, -48], [192, 192, 0], "world_tex"),
        box_brush([0, -64, -48], [128, 0, 0], "world_tex"),
        box_brush([0, 128, -48], [128, 192, 0], "world_tex"),
    ];
    let pool = fluid(&[box_brush([0, 0, -48], [128, 128, -8], "water_tex")]);
    let before = compile_unclosed(&pit, std::slice::from_ref(&pool));
    let (_, after) = compile(&pit, &[pool]);
    assert_eq!(after.volumes[0].surface_faces.len(), 1);
    assert_eq!(before.to_bytes(), after.to_bytes());

    let wind = entity(
        "push_volume",
        "\"push\" \"8 0 0\"",
        &[box_brush([0, 0, 0], [64, 64, 64], "push_tex")],
    );
    let before = compile_unclosed(&distant_world(), std::slice::from_ref(&wind));
    let (_, after) = compile(&distant_world(), &[wind]);
    assert_eq!(before.to_bytes(), after.to_bytes());

    // The only fluid volume lies wholly inside the world, so it draws nothing.
    let rock = box_brush([-128, -128, -128], [128, 128, 128], "world_tex");
    let buried = fluid(&[box_brush([-32, -32, -32], [32, 32, 32], "water_tex")]);
    let before = compile_unclosed(std::slice::from_ref(&rock), std::slice::from_ref(&buried));
    let (_, after) = compile(&[rock], &[buried]);
    assert!(after.volumes[0].surface_faces.is_empty());
    assert_eq!(before.to_bytes(), after.to_bytes());
}

// The e24 fixture's seams are already closed before closure runs; closure
// keeps them so and moves nothing else.
#[test]
fn e24_fixture_stays_closed_and_unmoved() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/maps/e24-environment-test.map");
    let map = parse_map_file(&path, MapFormat::IdTech2).expect("e24 fixture parses");
    let encode = |close: bool| {
        let mut texture_names = TextureNamesSection { names: Vec::new() };
        let encode = if close {
            encode_environment_volumes_section
        } else {
            encode_environment_volumes_section_unclosed
        };
        encode(
            &map.environment_volumes,
            map.initial_gravity,
            &map.brush_volumes,
            &mut texture_names,
        )
    };
    let (before, after) = (encode(false), encode(true));
    assert_eq!(open_seams(&after), Vec::new());
    assert_nothing_else_moves(&before, &after, TOLERANCE);
}

// Regression guard: a level whose fluid faces have no seam to close keeps the
// id-52 bytes the compiler wrote before closure existed (blake3, captured
// from the push-cut-gated compiler).
#[test]
fn levels_with_no_open_seam_keep_their_bytes_from_before_closure() {
    let fixtures: [(&str, Vec<String>, Vec<String>); 3] = [
        (
            "15d0a40f7bb9a2db8080a1f22fa76366f7840280a2a7f804415d69410d4f318e",
            distant_world(),
            vec![fluid(&[box_brush([0, -32, 32], [64, 32, 96], "water_tex")])],
        ),
        (
            "2cd6164ceb3c16fc04119ca93c0951058c14d7e81cdd58034857f1ae7d2d9d03",
            distant_world(),
            vec![
                fluid(&[box_brush([0, -32, 32], [64, 32, 96], "water_tex")]),
                fluid(&[box_brush([256, -32, 32], [320, 32, 96], "lava_tex")]),
            ],
        ),
        (
            "f5f3b4b67889d0ecc3c26730649bb438b10203f82cdd2b13a2715a0518804faa",
            vec![box_brush([-128, -128, -64], [128, 128, 0], "world_tex")],
            vec![fluid(&[box_brush(
                [-32, -32, -32],
                [32, 32, 0],
                "water_tex",
            )])],
        ),
    ];
    for (hash, world, volumes) in fixtures {
        let (_, section) = compile(&world, &volumes);
        assert_eq!(blake3::hash(&section.to_bytes()).to_hex().as_str(), hash);
    }
}

// --- Exact polygon fixtures ----------------------------------------------

/// A side whose plane holds `polygon`, for a synthetic piece.
fn side_of(polygon: &[DVec3]) -> BrushSide {
    let geometry = FaceGeometry::from_polygon(polygon).expect("fixture polygon has area");
    BrushSide {
        vertices: polygon.to_vec(),
        normal: geometry.normal,
        distance: geometry.normal.dot(polygon[0]),
        texture: "water_tex".to_string(),
        tex_projection: TextureProjection::default(),
    }
}

/// Closure over `volumes` (each a list of polygons), returning the polygons
/// it leaves and what it did.
fn close_polygons(volumes: &[Vec<Vec<DVec3>>]) -> (Vec<Vec<Vec<DVec3>>>, ClosureStats) {
    let sides: Vec<Vec<BrushSide>> = volumes
        .iter()
        .map(|faces| faces.iter().map(|polygon| side_of(polygon)).collect())
        .collect();
    let mut pieces: Vec<Vec<SurfacePiece<'_>>> = volumes
        .iter()
        .zip(&sides)
        .map(|(faces, sides)| {
            faces
                .iter()
                .zip(sides)
                .map(|(polygon, side)| SurfacePiece {
                    vertices: polygon.clone(),
                    side,
                })
                .collect()
        })
        .collect();
    let stats = close_seams(&mut pieces);
    let closed = pieces
        .into_iter()
        .map(|faces| faces.into_iter().map(|piece| piece.vertices).collect())
        .collect();
    (closed, stats)
}

/// Open seams among `volumes`' polygons, judged as encoded.
fn polygon_seams(volumes: &[Vec<Vec<DVec3>>]) -> Vec<OpenSeam> {
    let faces: Vec<((usize, usize), Vec<DVec3>)> = volumes
        .iter()
        .enumerate()
        .flat_map(|(record, faces)| {
            faces.iter().enumerate().map(move |(face, polygon)| {
                (
                    (record, face),
                    polygon.iter().map(|&p| encoded(p)).collect(),
                )
            })
        })
        .collect();
    open_seams_among(&faces)
}

/// An up-facing rectangle at height 0 over x `x0..x1`, z `z0..z1`, wound
/// counter-clockwise seen from above.
fn rect(x0: f64, z0: f64, x1: f64, z1: f64) -> Vec<DVec3> {
    vec![
        DVec3::new(x0, 0.0, z0),
        DVec3::new(x0, 0.0, z1),
        DVec3::new(x1, 0.0, z1),
        DVec3::new(x1, 0.0, z0),
    ]
}

/// An up-facing triangle with a right angle at (`x0`, `z0`), wound
/// counter-clockwise seen from above.
fn triangle(x0: f64, z0: f64, x1: f64, z1: f64) -> Vec<DVec3> {
    vec![
        DVec3::new(x0, 0.0, z0),
        DVec3::new(x0, 0.0, z1),
        DVec3::new(x1, 0.0, z0),
    ]
}

/// Asserts the polygons leave a seam open before closure, none after, and
/// each keeps its shape ([`assert_face_kept`]); returns the closed polygons.
#[track_caller]
fn assert_polygons_close(volumes: &[Vec<Vec<DVec3>>], reach: f64) -> Vec<Vec<Vec<DVec3>>> {
    assert!(
        !polygon_seams(volumes).is_empty(),
        "the fixture leaves no seam open before closure"
    );
    let (closed, _) = close_polygons(volumes);
    assert_eq!(
        polygon_seams(&closed),
        Vec::new(),
        "closure leaves seams open"
    );
    for (was, now) in volumes.iter().flatten().zip(closed.iter().flatten()) {
        let encode = |polygon: &[DVec3]| polygon.iter().map(|&p| encoded(p)).collect::<Vec<_>>();
        assert_face_kept(&encode(was), &encode(now), reach);
    }
    closed
}

/// The vertices of `polygon` within the tolerance of `point`.
fn near(polygon: &[DVec3], point: DVec3) -> usize {
    polygon
        .iter()
        .filter(|p| p.distance(point) <= TOLERANCE)
        .count()
}

// P3: two corners within 0.1 mm, not equal, take one encoded position.
#[test]
fn near_coincident_corners_weld_to_one_position() {
    let offset = 0.05 * MILLI;
    let volumes = vec![
        vec![rect(0.0, 0.0, 1.0, 1.0)],
        vec![rect(1.0 + offset, 0.0, 2.0, 1.0)],
    ];
    let closed = assert_polygons_close(&volumes, TOLERANCE);
    let corner = |polygon: &[DVec3]| {
        *polygon
            .iter()
            .find(|p| p.distance(DVec3::new(1.0, 0.0, 0.0)) <= TOLERANCE)
            .expect("a corner beside x 1")
    };
    assert_eq!(
        encoded(corner(&closed[0][0])).to_array().map(f64::to_bits),
        encoded(corner(&closed[1][0])).to_array().map(f64::to_bits)
    );
}

// P4: a vertex near a neighbour's corner and its edge merges into the corner,
// with no extra vertex beside it.
#[test]
fn a_vertex_near_a_corner_and_an_edge_merges_into_the_corner() {
    let volumes = vec![
        vec![rect(0.0, 0.0, 1.0, 1.0)],
        vec![rect(1.0, 0.05 * MILLI, 2.0, 0.5)],
    ];
    let closed = assert_polygons_close(&volumes, TOLERANCE);
    assert_eq!(near(&closed[0][0], DVec3::new(1.0, 0.0, 0.0)), 1);
}

// P9: 0.15 mm from a neighbour's corner, or from its edge, is a gap, not a
// seam: closure leaves both faces as they were.
#[test]
fn vertices_past_the_tolerance_are_left_alone() {
    let gap = 0.15 * MILLI;
    for neighbour in [
        rect(1.0 + gap, 0.0, 2.0, 1.0),
        rect(1.0 + gap, 0.3, 2.0, 0.6),
    ] {
        let volumes = vec![vec![rect(0.0, 0.0, 1.0, 1.0)], vec![neighbour]];
        assert_eq!(polygon_seams(&volumes), Vec::new());
        let (closed, stats) = close_polygons(&volumes);
        assert_eq!(closed, volumes);
        assert_eq!((stats.welded, stats.inserted), (0, 0));
    }
}

/// A rectangle whose west edge sits `offset` east of `base`, touching the
/// edge x = `base` of a rectangle west of it along z 0.3..0.6.
fn edge_pair(base: f64, offset: f64) -> Vec<Vec<Vec<DVec3>>> {
    vec![
        vec![rect(base - 1.0, 0.0, base, 1.0)],
        vec![rect(base + offset, 0.3, base + 1.0, 0.6)],
    ]
}

// P16: just over 0.1 mm in f64, under it as encoded: closure judges as
// encoded, so it inserts.
#[test]
fn a_vertex_encoding_brings_within_the_tolerance_is_inserted() {
    // f32 steps 30.5 µm near 300 m, so 300 + 0.1001 mm encodes to 300 + 0.0916 mm.
    let (base, offset) = (300.0, 0.1001 * MILLI);
    assert!(offset > TOLERANCE);
    assert!(encoded(DVec3::splat(base + offset)).x - base <= TOLERANCE);
    let closed = assert_polygons_close(&edge_pair(base, offset), TOLERANCE);
    assert_eq!(
        closed[0][0].len(),
        6,
        "both neighbour corners join the edge"
    );
}

// P19: just under 0.1 mm in f64, over it as encoded: no seam, no insert.
#[test]
fn a_vertex_encoding_puts_past_the_tolerance_is_left_alone() {
    // f32 steps 61 µm near 1000 m, so 1000 + 0.0999 mm encodes to 1000 + 0.122 mm.
    let (base, offset) = (1000.0, 0.0999 * MILLI);
    assert!(offset < TOLERANCE);
    assert!(encoded(DVec3::splat(base + offset)).x - base > TOLERANCE);
    let volumes = edge_pair(base, offset);
    assert_eq!(polygon_seams(&volumes), Vec::new());
    let (closed, stats) = close_polygons(&volumes);
    assert_eq!(closed, volumes);
    assert_eq!(stats.inserted, 0);
}

// P10: a chain of corners 0.06 mm apart, 0.12 mm end to end, moves no vertex
// past the tolerance.
#[test]
fn a_weld_chain_moves_no_vertex_past_the_tolerance() {
    let step = 0.06 * MILLI;
    let volumes = vec![
        vec![rect(0.0, -1.0, 1.0, 0.0)],
        vec![rect(1.0 + step, -1.0, 2.0, 0.0)],
        vec![rect(1.0 + 2.0 * step, 0.0, 2.0, 1.0)],
    ];
    assert_polygons_close(&volumes, TOLERANCE);
}

// P21: five corners 0.06 mm apart, 0.24 mm end to end: the chain breaks
// rather than carrying any vertex past the tolerance.
#[test]
fn a_long_weld_chain_breaks_rather_than_drifting() {
    let step = 0.06 * MILLI;
    let volumes: Vec<Vec<Vec<DVec3>>> = (0..5)
        .map(|k| {
            let x = 1.0 + k as f64 * step;
            let z = if k % 2 == 0 { -1.0 } else { 1.0 };
            vec![if z < 0.0 {
                rect(x, z, x + 1.0, 0.0)
            } else {
                rect(x, 0.0, x + 1.0, z)
            }]
        })
        .collect();
    assert_polygons_close(&volumes, TOLERANCE);
}

// P11: two near-coincident corners on a third face's edge weld first, so the
// third face gains one vertex.
#[test]
fn two_near_corners_on_a_third_faces_edge_add_one_vertex() {
    let volumes = vec![
        vec![rect(0.0, -1.0, 2.0, 0.0)],
        vec![rect(0.0, 0.0, 1.0, 1.0)],
        vec![rect(1.0 + 0.05 * MILLI, 0.0, 2.0, 1.0)],
    ];
    let closed = assert_polygons_close(&volumes, TOLERANCE);
    assert_eq!(near(&closed[0][0], DVec3::new(1.0, 0.0, 0.0)), 1);
}

// P12: insert candidates are judged after the weld. A corner pair 0.14 and
// 0.06 mm north of a third face's edge welds to one of the two; mirrored, the
// other wins. The third face gains the welded corner only where it lands
// within the tolerance.
#[test]
fn inserts_are_judged_on_welded_positions() {
    let (far, close) = (0.14 * MILLI, 0.06 * MILLI);
    let mut gains = Vec::new();
    for mirror in [1.0, -1.0] {
        let volumes = vec![
            vec![if mirror > 0.0 {
                rect(0.0, -1.0, 2.0, 0.0)
            } else {
                rect(0.0, 0.0, 2.0, 1.0)
            }],
            vec![if mirror > 0.0 {
                rect(0.0, far, 1.0, 1.0)
            } else {
                rect(0.0, -1.0, 1.0, -far)
            }],
            vec![if mirror > 0.0 {
                rect(1.0, close, 2.0, 1.0)
            } else {
                rect(1.0, -1.0, 2.0, -close)
            }],
        ];
        let closed = assert_polygons_close(&volumes, TOLERANCE);
        let welded = *closed[1][0]
            .iter()
            .find(|p| (p.x - 1.0).abs() <= TOLERANCE && p.z.abs() < 1.0 * MILLI)
            .expect("the pair's corner");
        let third = &closed[0][0];
        let gained = third.iter().any(|p| encoded(*p) == encoded(welded));
        assert_eq!(gained, welded.z.abs() <= TOLERANCE, "{welded}");
        gains.push(gained);
    }
    gains.sort_unstable();
    assert_eq!(gains, [false, true], "each mirror welds the other way");
}

// P17: a weld never merges two vertices of one face; the neighbour holds both
// ends of a 0.05 mm sliver edge lying on its edge.
#[test]
fn a_sliver_keeps_its_corners_and_its_neighbour_holds_both() {
    let short = 0.05 * MILLI;
    for sliver in [
        triangle(0.5, 0.0, 0.5 + short, 1.0),
        triangle(1.0 - short, 0.0, 1.0, 1.0),
    ] {
        let volumes = vec![vec![rect(0.0, -1.0, 1.0, 0.0)], vec![sliver]];
        let closed = assert_polygons_close(&volumes, TOLERANCE);
        for corner in &closed[1][0] {
            if corner.z.abs() <= TOLERANCE {
                assert!(
                    closed[0][0].iter().any(|p| encoded(*p) == encoded(*corner)),
                    "{corner} missing"
                );
            }
        }
    }
}

// Regression (review), P8, P17: a greedy weld can flatten or flip a sliver. F's
// corners v1 and v2 lie 0.05 mm apart; a neighbour's corner w sorts first and
// founds an anchor, v1 joins it, and v2, barred from an anchor already holding
// F, stays. With w level with v2, F's three corners end on one line; with w
// above v2, F winds the other way. Either way F keeps its positions, and the
// inserts close the seam instead.
#[test]
fn a_sliver_the_weld_would_flatten_or_flip_keeps_its_shape() {
    let (v1, v2) = (DVec3::ZERO, DVec3::new(0.0, 0.0, 0.05 * MILLI));
    let sliver = vec![DVec3::new(-1.0, 0.0, 0.05 * MILLI), v1, v2];
    let before = FaceGeometry::from_polygon(&sliver).expect("the sliver has area");
    // Flattens, then flips.
    for w in [
        DVec3::new(-0.01 * MILLI, 0.0, 0.05 * MILLI),
        DVec3::new(-0.01 * MILLI, 0.0, 0.09 * MILLI),
    ] {
        assert!(w.distance(v1) <= TOLERANCE && w.distance(v2) <= TOLERANCE);
        // Its base is off-centre: a centroid exactly on the line through v1
        // and v2, which the neighbour gains, would give them one angle.
        let neighbour = vec![w, DVec3::new(-0.5, 0.0, 1.0), DVec3::new(0.7, 0.0, 1.0)];
        let closed = assert_polygons_close(&[vec![sliver.clone()], vec![neighbour]], TOLERANCE);
        let face: Vec<DVec3> = closed[0][0].iter().map(|&p| encoded(p)).collect();
        let mut distinct: Vec<[u64; 3]> = face
            .iter()
            .map(|p| p.to_array().map(f64::to_bits))
            .collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert!(distinct.len() >= 3, "{w}: {face:?}");
        let after = FaceGeometry::from_polygon(&face).expect("the sliver keeps a normal");
        assert!(after.normal.dot(before.normal) > 0.0, "{w}: {face:?}");
        for corner in [v1, v2] {
            assert!(
                face.iter().any(|p| *p == encoded(corner)),
                "{w}: the sliver moved {corner}: {face:?}"
            );
        }
    }
}

// P18: an insert bends its edge, and the corner the bend brings within the
// tolerance is inserted too. That corner lies 0.18 mm from the edge as it
// stood before closure.
#[test]
fn a_corner_an_insert_bends_the_edge_onto_is_inserted() {
    let mm = MILLI;
    let volumes = vec![
        vec![rect(0.0, -10.0 * mm, 10.0 * mm, 0.0)],
        vec![triangle(5.0 * mm, 0.09 * mm, 6.0 * mm, 10.0 * mm)],
        vec![triangle(3.85 * mm, 0.18 * mm, 4.85 * mm, 10.0 * mm)],
    ];
    let closed = assert_polygons_close(&volumes, 2.0 * TOLERANCE);
    let bent_onto = encoded(DVec3::new(4.85 * mm, 0.0, 0.18 * mm));
    assert!(closed[0][0].iter().any(|p| encoded(*p) == bent_onto));
}

// P20: a weld that moves an edge's endpoint judges the corners near that edge
// against it as it now stands.
#[test]
fn inserts_see_edges_as_the_weld_leaves_them() {
    let volumes = vec![
        vec![rect(0.0, -1.0, 1.0, 0.0)],
        vec![rect(1.0 - 0.01 * MILLI, 0.09 * MILLI, 2.0, 1.0)],
        vec![triangle(0.9, -0.05 * MILLI, 0.95, -1.0)],
    ];
    assert_polygons_close(&volumes, TOLERANCE);
}

// P14: a weld may tilt a narrow piece off up-facing; a 6 mm square stays up.
#[test]
fn a_weld_tilts_only_a_narrow_piece_off_up_facing() {
    let mm = MILLI;
    for (size, stays_up) in [(6.0 * mm, true), (1.0 * mm, false)] {
        // The neighbour's corner sorts first, so the piece's corner moves to
        // it: 0.09 mm up and 0.01 mm west.
        let volumes = vec![
            vec![vec![
                DVec3::new(-0.01 * mm, 0.09 * mm, 0.0),
                DVec3::new(-0.01 * mm, 0.09 * mm, -1.0),
                DVec3::new(-1.0, 0.09 * mm, -1.0),
                DVec3::new(-1.0, 0.09 * mm, 0.0),
            ]],
            vec![rect(0.0, 0.0, size, size)],
        ];
        let (closed, stats) = close_polygons(&volumes);
        assert!(stats.welded > 0);
        let piece = FaceGeometry::from_polygon(&closed[1][0]).expect("the piece keeps a normal");
        assert_eq!(piece.is_up_facing(), stays_up, "{size} m square");
    }
}

/// A triangle in the plane x = `x` with its apex `apex` on the y = 0 plane
/// and its base 1 m up: a neighbour touching an up-facing face at one corner.
fn standing_triangle(apex: DVec3) -> Vec<DVec3> {
    vec![
        apex,
        DVec3::new(apex.x, 1.0, apex.z - 0.2),
        DVec3::new(apex.x, 1.0, apex.z + 0.2),
    ]
}

// P22: two neighbours' corners 0.09 mm off one edge, on opposite sides of its
// line and far apart along it. Inserting either bends the edge so the other
// lies 0.12 mm from it; both are judged against the edge as the weld left it,
// so it gains both, in order along it, whichever neighbour comes first.
#[test]
fn an_edge_gains_corners_off_either_side_of_its_line_in_either_order() {
    let off = 0.09 * MILLI;
    let (b, c) = (DVec3::new(0.5, 0.0, off), DVec3::new(1.5, 0.0, -off));
    let edge = rect(0.0, -1.0, 2.0, 0.0);
    for neighbours in [[b, c], [c, b]] {
        let volumes = vec![
            vec![edge.clone()],
            vec![standing_triangle(neighbours[0])],
            vec![standing_triangle(neighbours[1])],
        ];
        let closed = assert_polygons_close(&volumes, TOLERANCE);
        let at = |point: DVec3| {
            closed[0][0]
                .iter()
                .position(|p| encoded(*p) == encoded(point))
                .unwrap_or_else(|| panic!("the edge lacks {point}"))
        };
        // The edge runs from (0, 0, 0), ring vertex 1, to (2, 0, 0).
        assert!(1 < at(b) && at(b) < at(c), "{:?}", closed[0][0]);
    }
}

// P23: a neighbour's corner within 0.1 mm of both edges at a sloped piece's
// 30° corner, 0.3 mm from that corner and a hair to either side of its
// bisector. It joins
// the edge whose sector about the piece's centroid holds it, once, so the
// boundary still turns one way round.
#[test]
fn a_corner_near_both_edges_of_an_acute_corner_joins_the_edge_its_sector_holds() {
    // A plane sloped 20° about the x axis, and a 30° corner at its origin.
    let (u, v) = (
        DVec3::X,
        DVec3::new(0.0, 20f64.to_radians().sin(), 20f64.to_radians().cos()),
    );
    let at = |s: f64, t: f64| DVec3::new(0.2, 0.3, 0.1) + u * s + v * t;
    let half = 15f64.to_radians().tan();
    let piece = vec![at(1.0, half), at(0.0, 0.0), at(1.0, -half)];
    let apex = at(0.0, 0.0);
    let normal = u.cross(v);
    // Each side of the bisector, so the ring's first edge is the wrong one once.
    for side in [1.0, -1.0] {
        let corner = at(0.3 * MILLI, side * 0.01 * MILLI);
        for (edge_start, edge_end) in [(piece[0], piece[1]), (piece[1], piece[2])] {
            assert!(segment_distance(edge_start, edge_end, corner) < TOLERANCE);
        }
        assert!(corner.distance(apex) > TOLERANCE);
        let neighbour = vec![
            corner,
            corner + normal * 0.5 + u * 0.1,
            corner + normal * 0.5 - u * 0.1,
        ];
        let closed = assert_polygons_close(&[vec![piece.clone()], vec![neighbour]], TOLERANCE);
        let gained = closed[0][0]
            .iter()
            .filter(|p| encoded(**p) == encoded(corner))
            .count();
        assert_eq!(gained, 1, "{:?}", closed[0][0]);
    }
}

// Regression (review), P23, owner ruling: a neighbour's corner P lies 0.09 mm
// off edge s→c and 0.22 mm from c, the end of a near-flat (174°) corner, but
// past the area centroid's ray through c. It joins c→e, the edge whose sector
// holds it, though that edge lies 0.22 mm off, so F's angle order holds; the
// nearest edge would put it before c.
#[test]
fn a_corner_past_a_near_flat_corners_ray_joins_the_edge_its_sector_holds() {
    let (s, c, e) = (
        DVec3::new(-0.01, 0.0, 0.0),
        DVec3::ZERO,
        DVec3::new(10.0, 0.0, -1.0),
    );
    let face = vec![s, c, e, DVec3::new(-0.01, 0.0, -1.0)];
    let p = DVec3::new(-0.2 * MILLI, 0.0, 0.09 * MILLI);
    assert!(segment_distance(s, c, p) <= TOLERANCE);
    assert!(p.distance(c) > 2.0 * TOLERANCE && segment_distance(c, e, p) > 2.0 * TOLERANCE);
    // Past the ray from the centroid through c, so in c→e's sector.
    let (centre, normal) = centroid_and_normal(&face);
    assert!((c - centre).cross(p - centre).dot(normal) > 0.0);

    let closed = assert_polygons_close(&[vec![face], vec![standing_triangle(p)]], TOLERANCE);
    let ring = &closed[0][0];
    let at = |point: DVec3| {
        let found: Vec<usize> = (0..ring.len())
            .filter(|&i| encoded(ring[i]) == encoded(point))
            .collect();
        assert_eq!(found.len(), 1, "{point} in {ring:?}");
        found[0]
    };
    assert_eq!((at(c) + 1, at(p) + 1), (at(p), at(e)), "{ring:?}");
}

/// A fluid box over x `x0..x1`, z 0..32, bounded on one side by the plane
/// through (0, 0) and (256, 2), 0.45° off the x axis: north of it (to y 64)
/// when `north`, else south of it (from y -64).
fn slanted_box(x0: i32, x1: i32, north: bool) -> String {
    if north {
        box_brush([x0, 0, 0], [x1, 64, 32], "water_tex").replace(
            "( 0 0 0 ) ( 0 0 1 ) ( 1 0 0 )",
            "( 0 0 0 ) ( 0 0 1 ) ( 256 2 0 )",
        )
    } else {
        box_brush([x0, -64, 0], [x1, 0, 32], "water_tex").replace(
            "( 0 0 0 ) ( 1 0 0 ) ( 0 0 1 )",
            "( 0 0 0 ) ( 256 2 0 ) ( 0 0 1 )",
        )
    }
}

// End to end through the renderer's packing (the grid's accepted limit
// before closure). Two volumes' tops meet along an edge 0.45° off a lattice
// plane, and the north volume's unequal siblings leave a T-vertex mid-edge,
// 0.1 mm from that plane. Before closure the south top lacks the T-vertex,
// so the plane crosses its edge 13 mm away and the two sides disagree; after
// it, both carry identical vertices along the edge.
#[test]
fn faces_meeting_at_a_shallow_angle_to_a_lattice_plane_share_their_gridded_edge() {
    use postretro_render_cpu::fluid_surface::{FluidGridLattice, pack_fluid_surfaces};

    let volumes = [
        fluid(&[slanted_box(0, 256, false)]),
        fluid(&[slanted_box(0, 128, true), slanted_box(128, 256, true)]),
    ];
    let t_vertex = engine([128.0, 1.0, 32.0]);
    let (start, end) = (engine([0.0, 0.0, 32.0]), engine([256.0, 2.0, 32.0]));
    // Planes 8 m apart, one 0.1 mm from the T-vertex on x.
    let lattice = FluidGridLattice::new([(t_vertex.x + 0.1 * MILLI) as f32, 0.0, 0.0], [16.0; 3])
        .expect("a usable lattice");
    // Each top's packed vertices on the shared edge, as f32 bits.
    let edge_vertices = |section: &EnvironmentVolumesSection| {
        let mesh = pack_fluid_surfaces(&section.volumes, &[], 1, Some(&lattice));
        assert!(mesh.grid.is_some());
        let mut index = 0;
        let mut per_volume = Vec::new();
        for record in &section.volumes {
            let mut on_edge = std::collections::BTreeSet::new();
            for face in &record.surface_faces {
                let top = face
                    .vertices
                    .iter()
                    .all(|v| (f64::from(v.position[1]) - t_vertex.y).abs() < 1e-6);
                if let Some(map) = mesh.face_drift_maps[index].filter(|_| top) {
                    let range =
                        map.vertex_start as usize..(map.vertex_start + map.vertex_count) as usize;
                    for vertex in &mesh.vertices[range] {
                        let point = DVec3::from(vertex.position.map(f64::from));
                        if segment_distance(start, end, point) <= 2.0 * TOLERANCE {
                            on_edge.insert(vertex.position.map(f32::to_bits));
                        }
                    }
                }
                index += 1;
            }
            per_volume.push(on_edge);
        }
        per_volume
    };
    let before = edge_vertices(&compile_unclosed(&distant_world(), &volumes));
    assert_ne!(
        before[0], before[1],
        "today's open T-junction gives the sides different crossings"
    );
    let (_, after) = compile(&distant_world(), &volumes);
    let after = edge_vertices(&after);
    assert!(after[0].contains(&t_vertex.as_vec3().to_array().map(f32::to_bits)));
    assert_eq!(after[0], after[1]);
}
