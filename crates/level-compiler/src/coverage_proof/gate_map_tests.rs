// Coverage proof on the gate maps' post-cut geometry: edges, unproven spans with their nearest faces, and time.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use std::fmt::Write as _;

use glam::{DVec3, Vec3};

use super::test_fixtures::one_permit;
use super::weld::WeldRule;
use super::{UnprovenSpan, prove_coverage};
use crate::gate_map_geometry::map_geometries;
use crate::geometry::GeometryResult;
use crate::ray_traversal::distance_squared_to_triangle;

/// Maps the proof runs on unless `POSTRETRO_COVERAGE_MAPS` names others
/// (comma-separated, under `content/dev/maps/`).
const GATE_MAPS: [&str; 3] = [
    "stress-warren-hallway-inspection",
    "stress-warren-hallway-inspection-mini",
    "movement-feel",
];

/// Unproven spans each map's report lists.
const LISTED_SPANS: usize = 20;
/// Nearest faces listed beside each span.
const NEAREST_FACES: usize = 4;

/// Run with `--release -- --ignored --nocapture`.
#[test]
#[ignore = "builds gate-map geometry cold; run on demand with --release --nocapture"]
fn coverage_proof_on_gate_maps() {
    let maps: Vec<String> = match std::env::var("POSTRETRO_COVERAGE_MAPS") {
        Ok(list) => list.split(',').map(|m| m.trim().to_owned()).collect(),
        Err(_) => GATE_MAPS.iter().map(|&m| m.to_owned()).collect(),
    };
    for map in maps {
        println!("{}", map_report(&map));
    }
}

fn map_report(map: &str) -> String {
    let geometries = map_geometries(map);
    let geometry = &geometries.post_cut;
    let brushes = &geometries.post_cut_brushes;
    let mut out = String::new();
    let _ = writeln!(out, "== coverage proof: {map} ==");
    let _ = writeln!(
        out,
        "geometry built in {:.3} s; post-cut faces {}, triangles {}",
        geometries.built_in.as_secs_f64(),
        geometry.face_index_ranges.len(),
        geometry.geometry.indices.len() / 3,
    );
    let winding = winding_census(geometry);
    let _ = writeln!(
        out,
        "triangle windings against the face normal: {} counter-clockwise, {} clockwise, {} slivers rounded off the face plane (widest {:.3} µm)",
        winding.agree,
        winding.flipped,
        winding.off_plane,
        winding.widest_off_plane * 1e6,
    );
    let unwelded = super::prove_welded(
        geometry,
        brushes,
        WeldRule {
            radius: 0.0,
            keep_triangles_whole: true,
        },
        &one_permit(),
    );
    let _ = writeln!(
        out,
        "weld off: unproven spans {} totalling {:.6} m",
        unwelded.unproven.len(),
        unwelded.unproven_length(),
    );
    let report = prove_coverage(geometry, brushes, &one_permit());
    let _ = writeln!(
        out,
        "edges {}, proven {}, unproven spans {} totalling {:.6} m; proof time {:.3} s",
        report.edges,
        report.edges_proven,
        report.unproven.len(),
        report.unproven_length(),
        report.elapsed.as_secs_f64(),
    );
    let mut listed: Vec<&UnprovenSpan> = report.unproven.iter().collect();
    listed.sort_by(|a, b| b.length.total_cmp(&a.length));
    for span in listed.into_iter().take(LISTED_SPANS) {
        let _ = writeln!(out, "  {}", describe_span(span, geometry, brushes));
        for (face, distance) in nearest_faces(span, geometry) {
            let _ = writeln!(
                out,
                "      {:.6} mm to {}",
                distance * 1e3,
                describe_face(face, geometry, brushes)
            );
        }
    }
    out
}

/// Non-degenerate triangles by how their winding's normal meets their
/// face's stored normal.
#[derive(Default)]
struct Winding {
    /// Within 8° of the stored normal.
    agree: usize,
    /// Within 8° of its opposite: wound clockwise.
    flipped: usize,
    /// Neither: a sliver whose `f32` corners rounding has moved off the
    /// face plane, with the widest such sliver's width.
    off_plane: usize,
    widest_off_plane: f64,
}

fn winding_census(geometry: &GeometryResult) -> Winding {
    let geom = &geometry.geometry;
    let mut census = Winding::default();
    for tri in geom.indices.chunks(3) {
        let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| &geom.vertices[i as usize]);
        let [pa, pb, pc] = [a, b, c].map(|v| Vec3::from(v.position).as_dvec3());
        let winding = (pb - pa).cross(pc - pa);
        if winding.length_squared() == 0.0 {
            continue;
        }
        let cos = winding
            .normalize()
            .dot(Vec3::from(a.decode_normal()).as_dvec3());
        if cos > 0.99 {
            census.agree += 1;
        } else if cos < -0.99 {
            census.flipped += 1;
        } else {
            let longest = [pb - pa, pc - pb, pa - pc]
                .map(DVec3::length)
                .into_iter()
                .fold(0.0, f64::max);
            census.off_plane += 1;
            census.widest_off_plane = census.widest_off_plane.max(winding.length() / longest);
        }
    }
    census
}

fn point(p: DVec3) -> String {
    format!("({:.6}, {:.6}, {:.6})", p.x, p.y, p.z)
}

fn describe_face(face: usize, geometry: &GeometryResult, brushes: &[Option<u32>]) -> String {
    let meta = &geometry.geometry.faces[face];
    let texture = geometry
        .texture_names
        .names
        .get(meta.texture_index as usize)
        .map_or("?", String::as_str);
    let brush = brushes[face].map_or("?".to_owned(), |b| b.to_string());
    let range = geometry.face_index_ranges[face];
    let first = geometry.geometry.indices[range.index_offset as usize];
    let normal = geometry.geometry.vertices[first as usize].decode_normal();
    format!(
        "face {face} [{texture}, brush {brush}, cell {}, normal ({:.3}, {:.3}, {:.3})]",
        meta.leaf_index, normal[0], normal[1], normal[2]
    )
}

fn describe_span(
    span: &UnprovenSpan,
    geometry: &GeometryResult,
    brushes: &[Option<u32>],
) -> String {
    format!(
        "span {:.6} m, {} → {}, on {}",
        span.length,
        point(span.endpoints[0]),
        point(span.endpoints[1]),
        describe_face(span.face as usize, geometry, brushes)
    )
}

/// The faces nearest the span's midpoint, its own included, by distance to
/// their nearest triangle.
fn nearest_faces(span: &UnprovenSpan, geometry: &GeometryResult) -> Vec<(usize, f64)> {
    let geom = &geometry.geometry;
    let mut best: Vec<(usize, f64)> = geometry
        .face_index_ranges
        .iter()
        .enumerate()
        .map(|(face, range)| {
            let start = range.index_offset as usize;
            let distance = geom.indices[start..start + range.index_count as usize]
                .chunks(3)
                .map(|tri| {
                    let corners = [tri[0], tri[1], tri[2]]
                        .map(|i| Vec3::from(geom.vertices[i as usize].position).as_dvec3());
                    distance_squared_to_triangle(span.midpoint, corners)
                })
                .filter(|d| !d.is_nan())
                .fold(f64::INFINITY, f64::min)
                .sqrt();
            (face, distance)
        })
        .collect();
    best.sort_by(|a, b| a.1.total_cmp(&b.1));
    best.truncate(NEAREST_FACES);
    best
}
