// Face-cut tests: cut placement, sub-face order and coverage, shared cut vertices.
// See: context/lib/build_pipeline.md §Compiler pipeline (atlas preparation)

use glam::Vec3;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use super::*;
use crate::lightmap_bake::charts::plan_charts;

const PADDING: u32 = 2 * CHART_PADDING_TEXELS;

fn vertex(x: f32, z: f32) -> Vertex {
    Vertex::new(
        [x, 0.0, z],
        [x * 0.5, z * 0.25],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    )
}

/// Floor polygons (`y = 0`, facing `+Y`), each one fan-triangulated face of
/// leaf `face index`.
fn floor_faces(polygons: &[&[[f32; 2]]]) -> GeometryResult {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut faces = Vec::new();
    let mut face_index_ranges = Vec::new();
    for (face, polygon) in polygons.iter().enumerate() {
        let base = vertices.len() as u32;
        vertices.extend(polygon.iter().map(|&[x, z]| vertex(x, z)));
        let index_offset = indices.len() as u32;
        for i in 1..polygon.len() as u32 - 1 {
            indices.extend([base, base + i, base + i + 1]);
        }
        faces.push(FaceMeta {
            leaf_index: face as u32,
            texture_index: 0,
        });
        face_index_ranges.push(FaceIndexRange {
            index_offset,
            index_count: indices.len() as u32 - index_offset,
        });
    }
    GeometryResult {
        geometry: GeometrySection {
            vertices,
            indices,
            faces,
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges,
    }
}

fn square(x: f32, z: f32, size: f32) -> [[f32; 2]; 4] {
    [[x, z], [x + size, z], [x + size, z + size], [x, z + size]]
}

/// Area of a face's triangles.
fn face_area(geometry: &GeometryResult, face: usize) -> f32 {
    let range = geometry.face_index_ranges[face];
    let start = range.index_offset as usize;
    geometry.geometry.indices[start..start + range.index_count as usize]
        .chunks_exact(3)
        .map(|tri| {
            let p = |i: u32| Vec3::from(geometry.geometry.vertices[i as usize].position);
            (p(tri[1]) - p(tri[0]))
                .cross(p(tri[2]) - p(tri[0]))
                .length()
                * 0.5
        })
        .sum()
}

fn face_vertex_indices(geometry: &GeometryResult, face: usize) -> Vec<u32> {
    let range = geometry.face_index_ranges[face];
    let start = range.index_offset as usize;
    let mut indices = geometry.geometry.indices[start..start + range.index_count as usize].to_vec();
    indices.sort_unstable();
    indices.dedup();
    indices
}

// P5: a padded chart exactly at the pool edge is not cut; one texel over on
// one axis cuts that axis into exactly two windows that fit and differ by at
// most one texel, and leaves the other axis whole.
#[test]
fn chart_at_the_pool_edge_is_not_cut_and_one_texel_over_cuts_once() {
    let pool_edge = 2048;
    let at_edge = pool_edge - PADDING;
    assert_eq!(axis_cut_lines(at_edge, pool_edge), vec![0, at_edge]);

    let over = at_edge + 1;
    let lines = axis_cut_lines(over, pool_edge);
    assert_eq!(lines.len(), 3, "exactly two pieces");
    let windows = axis_windows(&lines);
    for window in &windows {
        assert!(
            window.len() as u32 + PADDING <= pool_edge,
            "{window:?} fits"
        );
    }
    let (a, b) = (windows[0].len(), windows[1].len());
    assert!(a.abs_diff(b) <= 1, "window extents {a} and {b}");

    // Through chart planning: a face 0.04 m past the edge on u only.
    let density = 0.04;
    let extent = over as f32 * density;
    let geometry = floor_faces(&[&[[0.0, 0.0], [extent, 0.0], [extent, 10.0], [0.0, 10.0]]]);
    let charts = plan_charts(&geometry, density, &[]).unwrap();
    assert_eq!(charts[0].width_texels, pool_edge + 1);
    let cuts = plan_face_cuts(&charts, pool_edge);
    assert_eq!(cuts.len(), 1);
    assert_eq!(cuts[0].u_lines.len(), 3, "u cut once");
    assert_eq!(cuts[0].v_lines.len(), 2, "v not cut");
}

#[test]
fn axis_cuts_use_the_fewest_near_equal_pieces_that_fit() {
    for (interior, pool_edge) in [(200, 64), (4096, 2048), (10_000, 2048), (61, 64)] {
        let lines = axis_cut_lines(interior, pool_edge);
        let windows = axis_windows(&lines);
        assert!(
            windows
                .iter()
                .all(|w| w.len() as u32 + PADDING <= pool_edge),
            "{interior}: {windows:?}"
        );
        let segments: Vec<u32> = lines.windows(2).map(|pair| pair[1] - pair[0]).collect();
        let (min, max) = (
            segments.iter().min().unwrap(),
            segments.iter().max().unwrap(),
        );
        assert!(max - min <= 1, "{interior}: segments {segments:?}");
        if segments.len() > 1 {
            // One piece fewer would not fit.
            let fewer = (segments.len() - 1) as u32;
            let overlap = if fewer == 2 { 1 } else { 2 } * CUT_OVERLAP_TEXELS;
            let longest = interior.div_ceil(fewer) + if fewer == 1 { 0 } else { overlap };
            assert!(
                longest + PADDING > pool_edge,
                "{interior}: {fewer} pieces would fit"
            );
        }
    }
}

/// A 9×9 m square (face 1) between two small squares, cut on both axes at a
/// 64-texel test pool edge.
fn cut_square() -> (GeometryResult, Vec<Chart>, CutFaces, Vec<FaceCut>) {
    let mut geometry = floor_faces(&[
        &square(-5.0, -5.0, 1.0),
        &square(0.0, 0.0, 9.0),
        &square(20.0, 20.0, 1.0),
    ]);
    let mut charts = plan_charts(&geometry, 0.1, &[]).unwrap();
    let cuts = plan_face_cuts(&charts, 64);
    assert_eq!(cuts.len(), 1);
    assert_eq!(cuts[0].face, 1);
    let cut = apply_face_cuts(&mut geometry, &mut charts, &cuts);
    (geometry, charts, cut, cuts)
}

// P6, P7: sub-faces take the parent's slot in window order (rows of v, then
// u), later faces shift, every sub-chart windows its parent's grid within
// the pool edge, and the sub-faces cover the parent exactly.
#[test]
fn cut_face_becomes_ordered_sub_faces_at_its_slot_covering_it_exactly() {
    let (geometry, charts, cut, cuts) = cut_square();
    let pieces = cuts[0].window_count();
    assert!(pieces >= 4, "cut on both axes: {pieces}");
    assert_eq!(cut.face_remap[0], 0..1);
    assert_eq!(cut.face_remap[1], 1..1 + pieces);
    assert_eq!(cut.face_remap[2], 1 + pieces..2 + pieces);
    assert_eq!(geometry.face_index_ranges.len(), charts.len());

    let expected: Vec<[u32; 2]> = cuts[0]
        .windows()
        .map(|([u, v], _)| [u.start, v.start])
        .collect();
    let origins: Vec<[u32; 2]> = charts[1..1 + pieces]
        .iter()
        .map(|chart| chart.window.expect("sub-chart").origin)
        .collect();
    assert_eq!(origins, expected, "rows of v, then u");
    for chart in &charts[1..1 + pieces] {
        assert!(chart.width_texels <= 64 && chart.height_texels <= 64);
        assert_eq!(chart.leaf_index, 1, "sub-faces stay in the parent's leaf");
    }
    assert!(charts[0].window.is_none() && charts[1 + pieces].window.is_none());

    let total: f32 = (1..1 + pieces).map(|face| face_area(&geometry, face)).sum();
    assert!(
        (total - 81.0).abs() < 1.0e-3,
        "sub-faces cover the parent: {total}"
    );
}

// A non-rectangular face cut on both axes: a window its triangle misses
// emits nothing, and the rest still cover it exactly.
#[test]
fn window_a_polygon_misses_emits_no_sub_face() {
    let triangle: &[[f32; 2]] = &[[0.0, 0.0], [9.0, 0.0], [0.0, 9.0]];
    let mut geometry = floor_faces(&[triangle]);
    let mut charts = plan_charts(&geometry, 0.1, &[]).unwrap();
    let cuts = plan_face_cuts(&charts, 64);
    let cut = apply_face_cuts(&mut geometry, &mut charts, &cuts);
    let emitted = cut.face_remap[0].len();
    assert!(
        emitted < cuts[0].window_count(),
        "the far corner window is empty"
    );
    let total: f32 = (0..emitted).map(|face| face_area(&geometry, face)).sum();
    assert!((total - 40.5).abs() < 1.0e-3, "{total}");
}

// P8, P20: no vertex is shared between sibling sub-faces, and a vertex on a
// cut has bit-identical position and texture UV in both faces sharing it.
#[test]
fn sibling_sub_faces_own_their_vertices_and_agree_on_cut_vertices() {
    let (geometry, _, cut, _) = cut_square();
    let siblings: Vec<Vec<u32>> = cut.face_remap[1]
        .clone()
        .map(|face| face_vertex_indices(&geometry, face))
        .collect();
    for (i, a) in siblings.iter().enumerate() {
        for b in &siblings[i + 1..] {
            assert!(
                a.iter().all(|index| !b.contains(index)),
                "shared vertex index"
            );
        }
    }

    // Every interior cut vertex appears, with identical bits, in another
    // sub-face.
    let vertex = |index: u32| &geometry.geometry.vertices[index as usize];
    let bits = |index: u32| {
        let v = vertex(index);
        (v.position.map(f32::to_bits), v.uv.map(f32::to_bits))
    };
    let interior_corner =
        |index: u32| vertex(index).position[0] > 0.0 && vertex(index).position[0] < 9.0;
    let mut matched = 0;
    for (i, own) in siblings.iter().enumerate() {
        for &index in own.iter().filter(|&&index| interior_corner(index)) {
            let twins = siblings
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .filter(|(_, other)| other.iter().any(|&o| bits(o) == bits(index)))
                .count();
            assert!(
                twins >= 1,
                "cut vertex {:?} has no twin",
                vertex(index).position
            );
            matched += 1;
        }
    }
    assert!(matched > 0);
}

#[test]
fn applying_no_cuts_leaves_geometry_and_charts_untouched() {
    let mut geometry = floor_faces(&[&square(0.0, 0.0, 1.0), &square(3.0, 0.0, 1.0)]);
    let mut charts = plan_charts(&geometry, 0.1, &[]).unwrap();
    let (before_vertices, before_indices) = (
        geometry.geometry.vertices.clone(),
        geometry.geometry.indices.clone(),
    );
    let cuts = plan_face_cuts(&charts, 2048);
    assert!(cuts.is_empty());
    let cut = apply_face_cuts(&mut geometry, &mut charts, &cuts);
    assert_eq!(cut.face_remap, vec![0..1, 1..2]);
    assert_eq!(geometry.geometry.vertices, before_vertices);
    assert_eq!(geometry.geometry.indices, before_indices);
}
