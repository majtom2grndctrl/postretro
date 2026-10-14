//! Fluid surface contracts: buried, flush and sibling-shared faces draw nothing;
//! partly buried faces keep only their exposed part. Faces split along push
//! brushes where their 2 mm drift sample enters the push; a volume a push cut
//! has no T-junction, and one no push cut keeps its subtraction output. The
//! e24 fixture's pools sample the pushes over them.

use glam::DVec3;
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentVolumeRecord, EnvironmentVolumesSection,
};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::map_data::MapData;
use crate::map_format::MapFormat;
use crate::parse::parse_map_file;

use super::encode_environment_volumes_section;

/// Quake map units to engine meters.
const UNIT: f64 = 0.0254;
/// Position tolerance, meters: the parser widens f32 map coordinates.
const EPSILON: f64 = 1e-4;

/// A Quake axis-aligned box brush spanning `min`..`max` (map units), each face
/// wound so its normal points out of the box.
fn box_brush(min: [i32; 3], max: [i32; 3], texture: &str) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {texture} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {texture} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {texture} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {texture} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {texture} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {texture} 0 0 0 1 1\n\
         }}"
    )
}

fn entity(classname: &str, properties: &str, brushes: &[String]) -> String {
    let mut lines = vec!["{".to_string(), format!("\"classname\" \"{classname}\"")];
    lines.extend(properties.lines().map(str::to_owned));
    lines.extend(brushes.iter().cloned());
    lines.push("}".to_string());
    lines.join("\n")
}

fn fluid(brushes: &[String]) -> String {
    entity("fluid_volume", "\"fluid\" \"water\"", brushes)
}

/// Worldspawn with `world` brushes, then `volumes` as entities 1, 2, …
fn compile(world: &[String], volumes: &[String]) -> (MapData, EnvironmentVolumesSection) {
    static NEXT_MAP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let map_text = format!(
        "{}\n{}\n",
        entity("worldspawn", "", world),
        volumes.join("\n")
    );
    let path = std::env::temp_dir().join(format!(
        "postretro-fluid-surface-{}-{}.map",
        std::process::id(),
        NEXT_MAP.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    std::fs::write(&path, map_text).expect("write fixture map");
    let map = parse_map_file(&path, MapFormat::IdTech2);
    let _ = std::fs::remove_file(&path);
    let map = map.expect("fixture map parses");
    let mut texture_names = TextureNamesSection {
        names: vec!["world_tex".to_string()],
    };
    let section = encode_environment_volumes_section(
        &map.environment_volumes,
        -9.81,
        &map.brush_volumes,
        &mut texture_names,
    );
    (map, section)
}

/// Engine-space point of a Quake map-unit position.
fn engine(quake: [f64; 3]) -> DVec3 {
    DVec3::new(-quake[1], quake[2], -quake[0]) * UNIT
}

/// Quake map-unit position of an engine-space point.
fn quake(position: [f32; 3]) -> DVec3 {
    let p = DVec3::from(position.map(f64::from));
    DVec3::new(-p.z, -p.x, p.y) / UNIT
}

/// Outward Quake-axis label of a face, from its winding: counter-clockwise
/// around the outward normal, as static world faces are wound.
fn facing(face: &EnvironmentSurfaceFace) -> &'static str {
    let points: Vec<DVec3> = face.vertices.iter().map(|v| quake(v.position)).collect();
    let mut normal = DVec3::ZERO;
    for i in 0..points.len() {
        normal += points[i].cross(points[(i + 1) % points.len()]);
    }
    let normal = normal.normalize();
    let axes = [
        (DVec3::X, "+x"),
        (DVec3::NEG_X, "-x"),
        (DVec3::Y, "+y"),
        (DVec3::NEG_Y, "-y"),
        (DVec3::Z, "+z"),
        (DVec3::NEG_Z, "-z"),
    ];
    axes.iter()
        .find(|(axis, _)| normal.dot(*axis) > 1.0 - 1e-6)
        .map(|(_, label)| *label)
        .unwrap_or_else(|| panic!("face is not axis aligned: {normal:?}"))
}

/// Area in square map units.
fn area(face: &EnvironmentSurfaceFace) -> f64 {
    let points: Vec<DVec3> = face.vertices.iter().map(|v| quake(v.position)).collect();
    let mut twice = DVec3::ZERO;
    for i in 0..points.len() {
        twice += points[i].cross(points[(i + 1) % points.len()]);
    }
    0.5 * twice.length()
}

/// Summed area per outward axis, in square map units.
fn area_by_facing(
    record: &EnvironmentVolumeRecord,
) -> std::collections::BTreeMap<&'static str, f64> {
    let mut areas = std::collections::BTreeMap::new();
    for face in &record.surface_faces {
        *areas.entry(facing(face)).or_insert(0.0) += area(face);
    }
    areas
}

#[track_caller]
fn assert_areas(record: &EnvironmentVolumeRecord, expected: &[(&str, f64)]) {
    let areas = area_by_facing(record);
    let labels: Vec<&str> = areas.keys().copied().collect();
    let mut expected_labels: Vec<&str> = expected.iter().map(|(label, _)| *label).collect();
    expected_labels.sort_unstable();
    assert_eq!(labels, expected_labels, "faces drawn: {areas:?}");
    for (label, want) in expected {
        let got = areas[label];
        assert!(
            (got - want).abs() < 1e-2,
            "{label}: area {got}, expected {want}"
        );
    }
}

/// A brush far from every volume, so the world is never empty.
fn distant_world() -> Vec<String> {
    vec![box_brush(
        [2000, 2000, 2000],
        [2064, 2064, 2064],
        "world_tex",
    )]
}

#[test]
fn floating_fluid_block_emits_all_six_faces_wound_outward() {
    let (_, section) = compile(
        &distant_world(),
        &[fluid(&[box_brush([0, -32, 32], [64, 32, 96], "water_tex")])],
    );
    let record = &section.volumes[0];
    assert_areas(
        record,
        &[
            ("+x", 64.0 * 64.0),
            ("-x", 64.0 * 64.0),
            ("+y", 64.0 * 64.0),
            ("-y", 64.0 * 64.0),
            ("+z", 64.0 * 64.0),
            ("-z", 64.0 * 64.0),
        ],
    );
    // Texel UVs come from the side's own projection, as for world faces: a
    // Standard-projected floor/ceiling face maps Quake (x, y) to (x, -y).
    for face in record.surface_faces.iter().filter(|f| facing(f) == "+z") {
        for vertex in &face.vertices {
            let q = quake(vertex.position);
            assert!((f64::from(vertex.uv[0]) - q.x).abs() < 1e-2, "{vertex:?}");
            assert!((f64::from(vertex.uv[1]) + q.y).abs() < 1e-2, "{vertex:?}");
        }
    }
}

#[test]
fn gravity_and_push_volumes_emit_no_surface() {
    let brush = box_brush([0, -32, 32], [64, 32, 96], "volume_tex");
    let (_, section) = compile(
        &distant_world(),
        &[
            entity(
                "gravity_volume",
                "\"gravity\" \"-3\"",
                std::slice::from_ref(&brush),
            ),
            entity(
                "push_volume",
                "\"push\" \"0 0 15\"",
                std::slice::from_ref(&brush),
            ),
        ],
    );
    assert_eq!(section.volumes.len(), 2);
    assert!(section.volumes.iter().all(|v| v.surface_faces.is_empty()));
}

/// One face flush against a solid wall and one shared with a sibling
/// brush of the same entity draw nothing; every other face draws whole.
#[test]
fn fluid_faces_flush_against_solid_or_shared_with_a_sibling_emit_no_surface() {
    let wall = box_brush([-32, -128, -128], [0, 128, 256], "world_tex");
    let (_, section) = compile(
        &[wall],
        &[fluid(&[
            box_brush([0, -32, 32], [64, 32, 96], "water_tex"),
            box_brush([64, -32, 32], [128, 32, 96], "water_tex"),
        ])],
    );
    let record = &section.volumes[0];
    // Brush A's -x is on the wall; A's +x and B's -x are the shared face. The
    // union is 128 x 64 x 64 with its -x face buried.
    assert_areas(
        record,
        &[
            ("+x", 64.0 * 64.0),
            ("+y", 128.0 * 64.0),
            ("-y", 128.0 * 64.0),
            ("+z", 128.0 * 64.0),
            ("-z", 128.0 * 64.0),
        ],
    );
    assert_eq!(record.surface_faces.len(), 9, "A draws 4 faces, B draws 5");
    for face in &record.surface_faces {
        let on_shared_plane = face
            .vertices
            .iter()
            .all(|v| (quake(v.position).x - 64.0).abs() < 1e-2);
        assert!(!on_shared_plane, "no face on the sibling-shared plane");
    }
}

/// A face partly flush against solid draws only its exposed part, and no
/// piece reaches into the buried region.
#[test]
fn fluid_face_partly_flush_against_solid_emits_only_its_exposed_part() {
    // The wall covers the fluid's -x face only below z = 64.
    let wall = box_brush([-32, -128, -128], [0, 128, 64], "world_tex");
    let (_, section) = compile(
        &[wall],
        &[fluid(&[box_brush([0, -32, 32], [64, 32, 96], "water_tex")])],
    );
    let record = &section.volumes[0];
    assert_areas(
        record,
        &[
            ("+x", 64.0 * 64.0),
            ("-x", 64.0 * 32.0),
            ("+y", 64.0 * 64.0),
            ("-y", 64.0 * 64.0),
            ("+z", 64.0 * 64.0),
            ("-z", 64.0 * 64.0),
        ],
    );
    for face in record.surface_faces.iter().filter(|f| facing(f) == "-x") {
        for vertex in &face.vertices {
            let q = quake(vertex.position);
            assert!(q.z >= 64.0 - 1e-2, "buried region leaked: {q:?}");
            let expected = engine([0.0, q.y, q.z]);
            assert!(
                (DVec3::from(vertex.position.map(f64::from)) - expected).length() < EPSILON,
                "exposed piece stays on the face plane"
            );
        }
    }
}

/// A pool in a pit carved into the floor draws its top; its bottom and sides
/// rest flush on solid.
#[test]
fn pool_sunk_into_a_floor_emits_its_top_but_not_its_buried_bottom_or_sides() {
    let world = vec![
        box_brush([-64, -64, -64], [192, 192, -48], "world_tex"),
        box_brush([-64, -64, -48], [0, 192, 0], "world_tex"),
        box_brush([128, -64, -48], [192, 192, 0], "world_tex"),
        box_brush([0, -64, -48], [128, 0, 0], "world_tex"),
        box_brush([0, 128, -48], [128, 192, 0], "world_tex"),
    ];
    let (_, section) = compile(
        &world,
        &[fluid(&[box_brush(
            [0, 0, -48],
            [128, 128, -8],
            "water_tex",
        )])],
    );
    assert_areas(&section.volumes[0], &[("+z", 128.0 * 128.0)]);
}

/// A fluid block sunk into an uncarved slab keeps only what stands above it:
/// the whole top, each side clipped at the slab's top, and no bottom.
#[test]
fn fluid_block_sunk_into_solid_keeps_only_its_parts_above_the_solid() {
    let slab = box_brush([-128, -128, -64], [192, 192, 0], "world_tex");
    let (_, section) = compile(
        &[slab],
        &[fluid(&[box_brush([0, 0, -32], [64, 64, 32], "water_tex")])],
    );
    let record = &section.volumes[0];
    assert_areas(
        record,
        &[
            ("+x", 64.0 * 32.0),
            ("-x", 64.0 * 32.0),
            ("+y", 64.0 * 32.0),
            ("-y", 64.0 * 32.0),
            ("+z", 64.0 * 64.0),
        ],
    );
    for face in &record.surface_faces {
        for vertex in &face.vertices {
            assert!(
                quake(vertex.position).z >= -1e-2,
                "no piece inside the slab"
            );
        }
    }
}

/// Overlapping sibling brushes share a top plane: the overlap draws once, with
/// no hole and no double layer, and no face lies inside the union.
#[test]
fn overlapping_sibling_brushes_draw_their_union_boundary_once() {
    let (_, section) = compile(
        &distant_world(),
        &[fluid(&[
            box_brush([0, 0, 0], [96, 64, 64], "water_tex"),
            box_brush([64, 0, 0], [160, 64, 64], "water_tex"),
        ])],
    );
    assert_areas(
        &section.volumes[0],
        &[
            ("+x", 64.0 * 64.0),
            ("-x", 64.0 * 64.0),
            ("+y", 160.0 * 64.0),
            ("-y", 160.0 * 64.0),
            ("+z", 160.0 * 64.0),
            ("-z", 160.0 * 64.0),
        ],
    );
}

/// Known limit: two fluid entities do not cull each other, so each draws the
/// face they share.
#[test]
fn adjacent_fluid_entities_each_draw_their_shared_face() {
    let (_, section) = compile(
        &distant_world(),
        &[
            fluid(&[box_brush([0, 0, 0], [64, 64, 64], "water_tex")]),
            fluid(&[box_brush([64, 0, 0], [128, 64, 64], "water_tex")]),
        ],
    );
    assert_eq!(section.volumes[0].surface_faces.len(), 6);
    assert_eq!(section.volumes[1].surface_faces.len(), 6);
}

/// The shared level texture table: a texture the world already uses keeps its
/// index, and a fluid-only texture appends after every world entry.
#[test]
fn fluid_textures_reuse_world_indices_and_append_new_ones_after_the_world() {
    let (map, _) = compile(
        &distant_world(),
        &[
            fluid(&[box_brush([0, 0, 0], [64, 64, 64], "world_tex")]),
            fluid(&[box_brush([200, 0, 0], [264, 64, 64], "water_tex")]),
        ],
    );
    let mut texture_names = TextureNamesSection {
        names: vec!["mover_tex".to_string(), "world_tex".to_string()],
    };
    let section = encode_environment_volumes_section(
        &map.environment_volumes,
        -9.81,
        &map.brush_volumes,
        &mut texture_names,
    );
    assert_eq!(texture_names.names, ["mover_tex", "world_tex", "water_tex"]);
    assert!(
        section.volumes[0]
            .surface_faces
            .iter()
            .all(|face| face.texture_index == 1)
    );
    assert!(
        section.volumes[1]
            .surface_faces
            .iter()
            .all(|face| face.texture_index == 2)
    );
}

fn push(push: &str, brushes: &[String]) -> String {
    entity("push_volume", &format!("\"push\" \"{push}\""), brushes)
}

/// The +z faces of a record, as (area in square map units, Quake centroid).
fn top_parts(record: &EnvironmentVolumeRecord) -> Vec<(f64, DVec3)> {
    let mut parts: Vec<(f64, DVec3)> = record
        .surface_faces
        .iter()
        .filter(|face| facing(face) == "+z")
        .map(|face| {
            let centroid = face
                .vertices
                .iter()
                .map(|v| quake(v.position))
                .sum::<DVec3>()
                / face.vertices.len() as f64;
            (area(face), centroid)
        })
        .collect();
    parts.sort_by(|a, b| a.1.x.total_cmp(&b.1.x));
    parts
}

#[track_caller]
fn assert_part_areas(parts: &[(f64, DVec3)], expected: &[f64]) {
    let areas: Vec<f64> = parts.iter().map(|(area, _)| *area).collect();
    assert_eq!(areas.len(), expected.len(), "parts: {parts:?}");
    for (got, want) in areas.iter().zip(expected) {
        assert!(
            (got - want).abs() < 1e-2,
            "areas {areas:?}, expected {expected:?}"
        );
    }
}

/// A fluid box spanning Quake x 0..128, y 0..64, z 0..64 (waterline z 64).
fn channel() -> String {
    fluid(&[box_brush([0, 0, 0], [128, 64, 64], "water_tex")])
}

#[test]
fn a_push_over_half_a_fluid_face_splits_it_at_the_push_boundary() {
    // The FGD's coincident current: the push's top is flush with the waterline.
    let (_, section) = compile(
        &distant_world(),
        &[
            channel(),
            push("0 8 0", &[box_brush([0, 0, 0], [64, 64, 64], "push_tex")]),
        ],
    );
    let parts = top_parts(&section.volumes[0]);
    assert_part_areas(&parts, &[64.0 * 64.0, 64.0 * 64.0]);
    assert!(parts[0].1.x < 64.0 && parts[1].1.x > 64.0);
}

// A push whose bottom sits on the waterline meets the face from outside.
#[test]
fn a_push_resting_on_the_waterline_from_above_leaves_the_face_whole() {
    let (_, unpushed) = compile(&distant_world(), &[channel()]);
    let (_, section) = compile(
        &distant_world(),
        &[
            channel(),
            push("8 0 0", &[box_brush([0, 0, 64], [64, 64, 128], "push_tex")]),
        ],
    );
    assert_part_areas(&top_parts(&section.volumes[0]), &[128.0 * 64.0]);
    assert_eq!(section.volumes[0], unpushed.volumes[0]);
}

#[test]
fn a_push_covering_the_whole_fluid_leaves_every_face_unsplit() {
    let (_, unpushed) = compile(&distant_world(), &[channel()]);
    let (_, section) = compile(
        &distant_world(),
        &[
            channel(),
            push(
                "0 8 0",
                &[box_brush([-16, -16, -16], [144, 80, 64], "push_tex")],
            ),
        ],
    );
    assert_eq!(section.volumes[0], unpushed.volumes[0]);
}

// The face is cut by every push brush over it, not only the winner's.
#[test]
fn overlapping_pushes_split_a_face_into_parts_under_one_set_of_pushes_each() {
    let (_, section) = compile(
        &distant_world(),
        &[
            channel(),
            push("0 8 0", &[box_brush([0, 0, 0], [80, 64, 64], "push_tex")]),
            push(
                "0 -8 0",
                &[box_brush([48, 0, 0], [128, 64, 64], "push_tex")],
            ),
        ],
    );
    assert_part_areas(
        &top_parts(&section.volumes[0]),
        &[48.0 * 64.0, 32.0 * 64.0, 48.0 * 64.0],
    );
}

// Touches bury nothing and slivers are never kept; parts tile the face.
#[test]
fn push_touches_and_sliver_overlaps_leave_the_face_whole() {
    let (_, unpushed) = compile(&distant_world(), &[channel()]);
    for brush in [
        // Edge touch: the push abuts the +x side from outside.
        box_brush([128, 0, 0], [192, 64, 64], "push_tex"),
        // Corner touch.
        box_brush([128, 64, 0], [192, 128, 64], "push_tex"),
    ] {
        let (_, section) = compile(&distant_world(), &[channel(), push("0 8 0", &[brush])]);
        assert_eq!(section.volumes[0], unpushed.volumes[0]);
    }

    // A corner overlap far under 1 mm².
    let sliver = "{\n\
        ( 127.99 0 0 ) ( 127.99 1 0 ) ( 127.99 0 1 ) push_tex 0 0 0 1 1\n\
        ( 192 0 0 ) ( 192 0 1 ) ( 192 1 0 ) push_tex 0 0 0 1 1\n\
        ( 0 -64 0 ) ( 0 -64 1 ) ( 1 -64 0 ) push_tex 0 0 0 1 1\n\
        ( 0 0.01 0 ) ( 1 0.01 0 ) ( 0 0.01 1 ) push_tex 0 0 0 1 1\n\
        ( 0 0 0 ) ( 1 0 0 ) ( 0 1 0 ) push_tex 0 0 0 1 1\n\
        ( 0 0 64 ) ( 0 1 64 ) ( 1 0 64 ) push_tex 0 0 0 1 1\n\
        }"
    .to_string();
    let (_, section) = compile(&distant_world(), &[channel(), push("0 8 0", &[sliver])]);
    let parts = top_parts(&section.volumes[0]);
    assert_eq!(
        parts.len(),
        1,
        "a sub-mm² overlap split the face: {parts:?}"
    );
    assert!((parts[0].0 - 128.0 * 64.0).abs() < 1e-2);
}

#[test]
fn a_thin_push_strip_splits_off_a_thin_part_and_the_parts_tile_the_face() {
    // A 0.1-unit (2.5 mm) strip along the face's +x edge: far above 1 mm².
    let strip = "{\n\
        ( 127.9 0 0 ) ( 127.9 1 0 ) ( 127.9 0 1 ) push_tex 0 0 0 1 1\n\
        ( 160 0 0 ) ( 160 0 1 ) ( 160 1 0 ) push_tex 0 0 0 1 1\n\
        ( 0 -16 0 ) ( 0 -16 1 ) ( 1 -16 0 ) push_tex 0 0 0 1 1\n\
        ( 0 80 0 ) ( 1 80 0 ) ( 0 80 1 ) push_tex 0 0 0 1 1\n\
        ( 0 0 -16 ) ( 1 0 -16 ) ( 0 1 -16 ) push_tex 0 0 0 1 1\n\
        ( 0 0 64 ) ( 0 1 64 ) ( 1 0 64 ) push_tex 0 0 0 1 1\n\
        }"
    .to_string();
    let (_, section) = compile(&distant_world(), &[channel(), push("0 8 0", &[strip])]);
    let parts = top_parts(&section.volumes[0]);
    assert_eq!(parts.len(), 2, "{parts:?}");
    assert!((parts[1].0 - 0.1 * 64.0).abs() < 1e-2, "{parts:?}");
    let total: f64 = parts.iter().map(|(area, _)| area).sum();
    // 1 mm² in square map units.
    assert!((total - 128.0 * 64.0).abs() < 1e-6 / (UNIT * UNIT));
}

/// Map units per millimetre.
const MM: f64 = 1.0 / 25.4;

/// [`box_brush`] at fractional map coordinates, for sub-unit offsets.
fn box_brush_at(min: [f64; 3], max: [f64; 3], texture: &str) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {texture} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {texture} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {texture} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {texture} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {texture} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {texture} 0 0 0 1 1\n\
         }}"
    )
}

/// The +z faces of a record, west to east by centroid.
fn top_faces(record: &EnvironmentVolumeRecord) -> Vec<&EnvironmentSurfaceFace> {
    let centroid_x = |face: &EnvironmentSurfaceFace| {
        face.vertices
            .iter()
            .map(|v| quake(v.position).x)
            .sum::<f64>()
            / face.vertices.len() as f64
    };
    let mut faces: Vec<&EnvironmentSurfaceFace> = record
        .surface_faces
        .iter()
        .filter(|face| facing(face) == "+z")
        .collect();
    faces.sort_by(|a, b| centroid_x(a).total_cmp(&centroid_x(b)));
    faces
}

/// Whether each top face, west to east, samples a push.
fn tops_sampling_a_push(section: &EnvironmentVolumesSection) -> Vec<bool> {
    top_faces(&section.volumes[0])
        .iter()
        .map(|face| !pushes_at(section, inside_sample(face)).is_empty())
        .collect()
}

// The face splits where its 2 mm drift sample is inside the push, not where
// the face itself is: a current whose top sits less than 2 mm under the
// waterline drifts the part it covers, and a deeper one drifts none of it.
#[test]
fn a_current_just_under_the_waterline_splits_the_face_where_its_drift_sample_is_inside() {
    for (depth_mm, splits) in [(0.5, true), (1.5, true), (2.5, false)] {
        let top = 64.0 - depth_mm * MM;
        let current = box_brush_at([0.0, 0.0, 0.0], [64.0, 64.0, top], "push_tex");
        let (_, section) = compile(&distant_world(), &[channel(), push("0 8 0", &[current])]);
        let record = &section.volumes[0];
        if splits {
            assert_part_areas(&top_parts(record), &[64.0 * 64.0, 64.0 * 64.0]);
            assert_eq!(
                tops_sampling_a_push(&section),
                [true, false],
                "{depth_mm} mm"
            );
        } else {
            assert_part_areas(&top_parts(record), &[128.0 * 64.0]);
            assert_eq!(tops_sampling_a_push(&section), [false], "{depth_mm} mm");
        }
        assert_parts_wholly_under_or_clear(&section, record);
        assert_eq!(t_junctions(record), Vec::<DVec3>::new(), "{depth_mm} mm");
    }
}

// A push resting on the water with its bottom sunk under the waterline drifts
// the face only once its bottom is deeper than the 2 mm drift sample.
#[test]
fn a_push_sunk_just_under_the_waterline_splits_the_face_only_past_the_drift_sample() {
    for (depth_mm, splits) in [(0.5, false), (1.5, false), (2.5, true)] {
        let bottom = 64.0 - depth_mm * MM;
        let wind = box_brush_at([0.0, 0.0, bottom], [64.0, 64.0, 128.0], "push_tex");
        let (_, section) = compile(&distant_world(), &[channel(), push("8 0 0", &[wind])]);
        let record = &section.volumes[0];
        if splits {
            assert_part_areas(&top_parts(record), &[64.0 * 64.0, 64.0 * 64.0]);
            assert_eq!(
                tops_sampling_a_push(&section),
                [true, false],
                "{depth_mm} mm"
            );
        } else {
            assert_part_areas(&top_parts(record), &[128.0 * 64.0]);
            assert_eq!(tops_sampling_a_push(&section), [false], "{depth_mm} mm");
        }
        assert_parts_wholly_under_or_clear(&section, record);
        assert_eq!(t_junctions(record), Vec::<DVec3>::new(), "{depth_mm} mm");
    }
}

// Two cuts leave one part's corner inside another part's edge, and on the
// edges of the sides the push does not reach; every face gains the corners
// lying on its edges.
#[test]
fn a_push_over_one_corner_leaves_no_vertex_inside_another_parts_edge() {
    let corner = box_brush([96, 48, 0], [160, 96, 64], "push_tex");
    let (_, section) = compile(&distant_world(), &[channel(), push("0 8 0", &[corner])]);
    let record = &section.volumes[0];
    let parts = top_parts(record);
    assert_eq!(parts.len(), 3, "two cuts make three top parts: {parts:?}");
    let total: f64 = parts.iter().map(|(area, _)| area).sum();
    assert!((total - 128.0 * 64.0).abs() < 1e-2, "{parts:?}");
    assert_eq!(t_junctions(record), Vec::<DVec3>::new());
    assert_parts_wholly_under_or_clear(&section, record);
}

#[test]
fn an_inset_current_flush_with_the_waterline_leaves_no_t_junction() {
    let inset = box_brush([32, 16, 0], [96, 48, 64], "push_tex");
    let (_, section) = compile(&distant_world(), &[channel(), push("0 8 0", &[inset])]);
    let record = &section.volumes[0];
    assert!(top_parts(record).len() > 1, "the current splits the top");
    assert_eq!(t_junctions(record), Vec::<DVec3>::new());
    assert_parts_wholly_under_or_clear(&section, record);
}

// The sunk push splits the sides under it at the waterline but leaves the top
// whole: the top gains the corners the split sides leave on its edges.
#[test]
fn a_whole_top_gains_the_corners_its_split_sides_leave_on_its_edges() {
    let wind = box_brush_at([0.0, 0.0, 64.0 - 0.5 * MM], [64.0, 64.0, 128.0], "push_tex");
    let (_, section) = compile(&distant_world(), &[channel(), push("8 0 0", &[wind])]);
    let record = &section.volumes[0];
    let tops = top_faces(record);
    assert_eq!(tops.len(), 1, "the top draws whole");
    let top: Vec<DVec3> = tops[0].vertices.iter().map(|v| quake(v.position)).collect();
    assert_eq!(
        top.len(),
        6,
        "four corners and one side corner on each long edge: {top:?}"
    );
    for corner in [DVec3::new(64.0, 0.0, 64.0), DVec3::new(64.0, 64.0, 64.0)] {
        assert!(
            top.iter().any(|vertex| vertex.distance(corner) < 1e-2),
            "{corner} missing from {top:?}"
        );
    }
    assert_eq!(t_junctions(record), Vec::<DVec3>::new());
    assert_parts_wholly_under_or_clear(&section, record);
}

/// Every face of `record` is a quad with each vertex on a corner of its own
/// bounds: subtraction's output, with no vertex inserted on an edge.
#[track_caller]
fn assert_subtraction_quads(record: &EnvironmentVolumeRecord) {
    for face in &record.surface_faces {
        let points: Vec<DVec3> = face.vertices.iter().map(|v| quake(v.position)).collect();
        assert_eq!(points.len(), 4, "a vertex was inserted: {points:?}");
        let (mut min, mut max) = (points[0], points[0]);
        for &point in &points {
            (min, max) = (min.min(point), max.max(point));
        }
        for point in &points {
            for axis in 0..3 {
                let at = |bound: f64| (point[axis] - bound).abs() < 1e-2;
                assert!(
                    at(min[axis]) || at(max[axis]),
                    "{point} is not a corner of {points:?}"
                );
            }
        }
    }
}

// Burial and unequal siblings leave corners on neighbouring edges, but no push
// cuts these volumes, so each face keeps exactly what subtraction produced and
// the section's bytes do not depend on push splitting.
#[test]
fn volumes_no_push_cuts_keep_their_subtraction_output_with_no_inserted_vertex() {
    // The wall covers the fluid's -x face only below z = 64.
    let wall = box_brush([-32, -128, -128], [0, 128, 64], "world_tex");
    let (_, buried) = compile(
        &[wall],
        &[fluid(&[box_brush([0, -32, 32], [64, 32, 96], "water_tex")])],
    );
    // Two brushes sharing a top plane, the east one twice as wide.
    let (_, siblings) = compile(
        &distant_world(),
        &[fluid(&[
            box_brush([0, 0, 0], [64, 64, 64], "water_tex"),
            box_brush([64, 0, 0], [128, 128, 64], "water_tex"),
        ])],
    );
    assert_areas(
        &siblings.volumes[0],
        &[
            ("+x", 128.0 * 64.0),
            ("-x", 2.0 * 64.0 * 64.0),
            ("+y", 2.0 * 64.0 * 64.0),
            ("-y", 2.0 * 64.0 * 64.0),
            ("+z", 3.0 * 64.0 * 64.0),
            ("-z", 3.0 * 64.0 * 64.0),
        ],
    );
    for record in [&buried.volumes[0], &siblings.volumes[0]] {
        assert!(
            !t_junctions(record).is_empty(),
            "the fixture leaves a corner on a neighbouring edge"
        );
        assert_subtraction_quads(record);
    }
}

// A map with pushes and no fluid has no surface faces to split.
#[test]
fn push_volumes_without_fluid_emit_no_surface_faces() {
    let wind = push("8 0 0", &[box_brush([0, 0, 0], [64, 64, 64], "push_tex")]);
    let (_, section) = compile(&distant_world(), &[wind]);
    assert!(section.volumes[0].surface_faces.is_empty());
}

/// How far inside the fluid drift samples a face's push, meters.
const SAMPLE_DEPTH: f64 = 0.002;

/// A face's engine-space vertex positions.
fn positions(face: &EnvironmentSurfaceFace) -> Vec<DVec3> {
    face.vertices
        .iter()
        .map(|v| DVec3::from(v.position.map(f64::from)))
        .collect()
}

/// A face's engine-space unit normal from its winding, pointing out of the
/// fluid.
fn outward_normal(face: &EnvironmentSurfaceFace) -> DVec3 {
    let points = positions(face);
    let mut normal = DVec3::ZERO;
    for i in 0..points.len() {
        normal += points[i].cross(points[(i + 1) % points.len()]);
    }
    normal.normalize()
}

/// Engine-space sample point 2 mm inside the fluid below a face's centroid,
/// where drift samples the push (the face normal points out of the fluid).
fn inside_sample(face: &EnvironmentSurfaceFace) -> DVec3 {
    let points = positions(face);
    let centroid = points.iter().sum::<DVec3>() / points.len() as f64;
    centroid - outward_normal(face) * SAMPLE_DEPTH
}

/// Every face of `record` lies wholly under or wholly clear of each push
/// brush in `section`, as the face's 2 mm inside samples see it, and the
/// centroid sample drift reads agrees: under when it is inside the push,
/// clear when it is not. Clear means one push plane has every vertex's sample
/// on or outside it, which decides it for the axis-aligned fixtures here.
#[track_caller]
fn assert_parts_wholly_under_or_clear(
    section: &EnvironmentVolumesSection,
    record: &EnvironmentVolumeRecord,
) {
    let brushes: Vec<Vec<(DVec3, f64)>> = section
        .volumes
        .iter()
        .filter(|other| other.push.is_some())
        .flat_map(|push| &push.brushes)
        .map(|brush| {
            brush
                .planes
                .iter()
                .map(|plane| {
                    (
                        DVec3::from(plane.normal.map(f64::from)),
                        f64::from(plane.distance),
                    )
                })
                .collect()
        })
        .collect();
    for face in &record.surface_faces {
        let normal = outward_normal(face);
        let samples: Vec<DVec3> = positions(face)
            .into_iter()
            .map(|position| position - normal * SAMPLE_DEPTH)
            .collect();
        let centroid = inside_sample(face);
        for planes in &brushes {
            let under = samples
                .iter()
                .all(|sample| planes.iter().all(|(n, d)| n.dot(*sample) - d <= EPSILON));
            let clear = planes
                .iter()
                .any(|(n, d)| samples.iter().all(|sample| n.dot(*sample) - d >= -EPSILON));
            let sampled_inside = planes.iter().all(|(n, d)| n.dot(centroid) - d <= 0.0);
            if sampled_inside {
                assert!(
                    under,
                    "a face sampling the push is not wholly under it: {face:?}"
                );
            } else {
                assert!(
                    clear,
                    "a face sampling no push is not wholly clear of it: {face:?}"
                );
            }
        }
    }
}

/// Every vertex of one face of `record` lying strictly inside another face's
/// edge, in engine space: a T-junction.
fn t_junctions(record: &EnvironmentVolumeRecord) -> Vec<DVec3> {
    let faces: Vec<Vec<DVec3>> = record.surface_faces.iter().map(positions).collect();
    let mut found = Vec::new();
    for (own, polygon) in faces.iter().enumerate() {
        for (other, vertices) in faces.iter().enumerate() {
            if other == own {
                continue;
            }
            for &vertex in vertices {
                for i in 0..polygon.len() {
                    let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
                    if vertex.distance(a) <= EPSILON || vertex.distance(b) <= EPSILON {
                        continue;
                    }
                    let along = (vertex - a).dot(b - a) / (b - a).length_squared();
                    if along > 0.0 && along < 1.0 && vertex.distance(a + (b - a) * along) <= EPSILON
                    {
                        found.push(vertex);
                    }
                }
            }
        }
    }
    found
}

fn record_contains(record: &EnvironmentVolumeRecord, point: DVec3) -> bool {
    record.brushes.iter().any(|brush| {
        brush.planes.iter().all(|plane| {
            DVec3::from(plane.normal.map(f64::from)).dot(point) - f64::from(plane.distance) <= 0.0
        })
    })
}

/// The push records whose brushes contain `point`.
fn pushes_at(section: &EnvironmentVolumesSection, point: DVec3) -> Vec<[f32; 3]> {
    section
        .volumes
        .iter()
        .filter(|record| record.push.is_some() && record_contains(record, point))
        .map(|record| record.push.unwrap())
        .collect()
}

// The e24 fixture's current channel samples its coincident push on every
// face, so no face of it is left still; the fluid-motion wing's pools split
// exactly where their pushes are.
#[test]
fn e24_fixture_current_and_wing_pools_sample_the_pushes_over_them() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/maps/e24-environment-test.map");
    let map = parse_map_file(&path, MapFormat::IdTech2).expect("e24 fixture parses");
    let mut texture_names = TextureNamesSection { names: Vec::new() };
    let section = encode_environment_volumes_section(
        &map.environment_volumes,
        map.initial_gravity,
        &map.brush_volumes,
        &mut texture_names,
    );
    let fluid_at = |quake_point: [f64; 3]| {
        section
            .volumes
            .iter()
            .find(|record| !record.fluid.is_empty() && record_contains(record, engine(quake_point)))
            .expect("a fluid record holds the point")
    };
    // `0 6 0` in map axes is a current toward map +Y: engine -X.
    let north = [-6.0f32, 0.0, 0.0];

    let channel = fluid_at([1360.0, 784.0, -64.0]);
    assert!(!channel.surface_faces.is_empty());
    for face in &channel.surface_faces {
        assert_eq!(pushes_at(&section, inside_sample(face)), vec![north]);
    }

    // The two-brush pool: its east brush's top face splits at the push edge
    // (x 1904); only the part west of it samples the push.
    let partial = fluid_at([1700.0, 544.0, -64.0]);
    let tops: Vec<&EnvironmentSurfaceFace> = partial
        .surface_faces
        .iter()
        .filter(|face| facing(face) == "+z")
        .collect();
    assert_eq!(
        tops.len(),
        3,
        "west top, east top's pushed part, east top's still part"
    );
    let pushed = tops
        .iter()
        .filter(|face| !pushes_at(&section, inside_sample(face)).is_empty())
        .count();
    assert_eq!(pushed, 2);

    // The bending river: every top part samples exactly one push, east then
    // north, and no surface is drawn between its two brushes.
    let river = fluid_at([2200.0, 512.0, -64.0]);
    let mut directions = std::collections::BTreeSet::new();
    for face in river
        .surface_faces
        .iter()
        .filter(|face| facing(face) == "+z")
    {
        let pushes = pushes_at(&section, inside_sample(face));
        assert_eq!(pushes.len(), 1, "each river part lies under one push");
        directions.insert(format!("{:?}", pushes[0]));
    }
    assert_eq!(
        directions.len(),
        2,
        "the river drifts along both legs: {directions:?}"
    );
    // The legs share the plane y 576 over x 2384..2512.
    for face in &river.surface_faces {
        let points: Vec<DVec3> = face.vertices.iter().map(|v| quake(v.position)).collect();
        let centroid_x = points.iter().map(|p| p.x).sum::<f64>() / points.len() as f64;
        let on_shared_plane = points.iter().all(|p| (p.y - 576.0).abs() < 1e-2);
        assert!(
            !(on_shared_plane && centroid_x > 2384.0),
            "a surface is drawn between the river's brushes: {points:?}"
        );
    }
}
