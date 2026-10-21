//! Fluid surface fixtures and the helpers their tests share. Burial, push
//! splitting and the e24 fixture live in the child modules.
//!
//! Fluid surface contracts: buried, flush and sibling-shared faces draw nothing;
//! partly buried faces keep only their exposed part. Faces split along push
//! brushes where their 2 mm drift sample enters the push. Seams between fluid
//! faces close level-wide. The e24 fixture's pools sample the pushes over
//! them.

use glam::DVec3;
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentVolumeRecord, EnvironmentVolumesSection,
};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::map_data::MapData;
use crate::map_format::MapFormat;
use crate::parse::parse_map_file;

use super::closure::{SEAM_TOLERANCE, boundary_distance};
use super::{encode_environment_volumes_section, encode_environment_volumes_section_unclosed};

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
    compile_with(world, volumes, true)
}

/// [`compile`], with its faces as they stand before seam closure.
fn compile_unclosed(world: &[String], volumes: &[String]) -> EnvironmentVolumesSection {
    compile_with(world, volumes, false).1
}

fn compile_with(
    world: &[String],
    volumes: &[String],
    close: bool,
) -> (MapData, EnvironmentVolumesSection) {
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
    let encode = if close {
        encode_environment_volumes_section
    } else {
        encode_environment_volumes_section_unclosed
    };
    let section = encode(
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

fn push(push: &str, brushes: &[String]) -> String {
    entity("push_volume", &format!("\"push\" \"{push}\""), brushes)
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

/// Engine-space sample point where drift samples a face's push, mirroring the
/// runtime: the face's area centroid, 2 mm inside the fluid (the face normal
/// points out of the fluid).
fn inside_sample(face: &EnvironmentSurfaceFace) -> DVec3 {
    let normal = postretro_render_cpu::fluid_surface::face_normal(face)
        .expect("a test face is not degenerate");
    let centroid = postretro_render_cpu::fluid_surface::face_area_centroid(face, normal);
    centroid - normal.as_dvec3() * SAMPLE_DEPTH
}

/// Every vertex of one face of `record` lying strictly inside another face's
/// edge, in engine space: a T-junction. Checks within one record only;
/// level-wide checks (`open_seams`) live in this file and `closure.rs`.
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

/// One vertex of a fluid face lying within the seam tolerance of another
/// fluid face's boundary without being, bit for bit, one of its vertices.
#[derive(Debug, Clone, Copy, PartialEq)]
struct OpenSeam {
    /// (record, face) of the face whose boundary the vertex lies near.
    face: (usize, usize),
    /// (record, face) of the face the vertex belongs to.
    from: (usize, usize),
    vertex: DVec3,
}

/// Every open seam in `section`, level-wide across records, judged on the
/// encoded positions with closure's own distance test.
fn open_seams(section: &EnvironmentVolumesSection) -> Vec<OpenSeam> {
    let faces: Vec<((usize, usize), Vec<DVec3>)> = section
        .volumes
        .iter()
        .enumerate()
        .flat_map(|(record, volume)| {
            volume
                .surface_faces
                .iter()
                .enumerate()
                .map(move |(face, surface)| ((record, face), positions(surface)))
        })
        .collect();
    open_seams_among(&faces)
}

/// [`open_seams`] over faces given as (record, face) and encoded positions.
fn open_seams_among(faces: &[((usize, usize), Vec<DVec3>)]) -> Vec<OpenSeam> {
    use std::collections::{HashMap, HashSet};

    /// Bucket edge, meters: coarse, as only candidates are bucketed.
    const CELL: f64 = 1.0;
    let cell = |point: DVec3| (point / CELL).floor().as_i64vec3().to_array();
    let mut buckets: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (index, (_, polygon)) in faces.iter().enumerate() {
        let (mut min, mut max) = (DVec3::INFINITY, DVec3::NEG_INFINITY);
        for &point in polygon {
            (min, max) = (min.min(point), max.max(point));
        }
        let ([x0, y0, z0], [x1, y1, z1]) = (cell(min - SEAM_TOLERANCE), cell(max + SEAM_TOLERANCE));
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    buckets.entry([x, y, z]).or_default().push(index);
                }
            }
        }
    }
    let key = |point: DVec3| point.to_array().map(f64::to_bits);
    let mut found = Vec::new();
    for (from, (from_id, vertices)) in faces.iter().enumerate() {
        for &vertex in vertices {
            let mut seen = HashSet::new();
            for &face in buckets.get(&cell(vertex)).into_iter().flatten() {
                if face == from || !seen.insert(face) {
                    continue;
                }
                let (face_id, polygon) = &faces[face];
                if boundary_distance(polygon, vertex) <= SEAM_TOLERANCE
                    && !polygon.iter().any(|&point| key(point) == key(vertex))
                {
                    found.push(OpenSeam {
                        face: *face_id,
                        from: *from_id,
                        vertex,
                    });
                }
            }
        }
    }
    found
}

mod burial;
mod closure;
mod closure_bounds;
mod closure_maps;
mod e24;
mod push_split;
