//! Seam closure: every fixture leaves a fluid seam open before closure, and
//! after it every face that meets another carries its vertices bit for bit,
//! within a volume and across volumes. Nothing else moves: face count, order,
//! texture and side projection hold, corners move at most the tolerance, and
//! no face degenerates. Sub-millimetre cases are built as exact polygons.

use super::*;
use crate::environment_volumes::closure::{ClosureStats, close_seams, encoded};
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

/// Closure kept every record's face count, order and texture, every face's
/// UVs on its own side's projection, its area within the tolerance times its
/// perimeter, three distinct vertices and a normal. Every corner before
/// closure has a vertex after it within the tolerance, and every vertex after
/// it lies within `reach` of the face's boundary before it.
#[track_caller]
fn assert_nothing_else_moves(
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
            let (old_points, new_points) = (positions(old), positions(new));
            for (point, vertex) in new_points.iter().zip(&new.vertices) {
                let want = projected_uv(old, *point);
                let got = vertex.uv.map(f64::from);
                assert!(
                    (got[0] - want[0]).abs() < 1e-2 && (got[1] - want[1]).abs() < 1e-2,
                    "UV {got:?} is not its side's projection {want:?} at {point}"
                );
                assert!(
                    boundary_distance(&old_points, *point) <= reach,
                    "{point} lies off the face's boundary before closure"
                );
            }
            for corner in &old_points {
                assert!(
                    new_points.iter().any(|p| p.distance(*corner) <= TOLERANCE),
                    "corner {corner} moved past the tolerance"
                );
            }
            let mut distinct = new_points.clone();
            distinct.sort_by(|a, b| {
                a.x.total_cmp(&b.x)
                    .then(a.y.total_cmp(&b.y))
                    .then(a.z.total_cmp(&b.z))
            });
            distinct.dedup();
            assert!(distinct.len() >= 3, "a face collapsed: {new_points:?}");
            let geometry = FaceGeometry::from_polygon(&new_points).expect("a face keeps a normal");
            let old_geometry =
                FaceGeometry::from_polygon(&old_points).expect("the fixture face has a normal");
            let perimeter: f64 = (0..old_points.len())
                .map(|i| old_points[i].distance(old_points[(i + 1) % old_points.len()]))
                .sum();
            assert!(
                (geometry.area - old_geometry.area).abs() <= TOLERANCE * perimeter,
                "area {} moved from {}",
                geometry.area,
                old_geometry.area
            );
        }
    }
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

/// Asserts the polygons leave a seam open before closure, none after, and no
/// vertex lies further than `reach` from its face's boundary before closure;
/// returns the closed polygons.
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
        let mut distinct: Vec<[u64; 3]> =
            now.iter().map(|p| p.to_array().map(f64::to_bits)).collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert!(distinct.len() >= 3, "a face collapsed: {now:?}");
        assert!(
            FaceGeometry::from_polygon(now).is_some(),
            "a face lost its normal"
        );
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
