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

// P20 past two pieces: a slanted face cut into many pieces on each axis.
// A middle piece clips both its lines; every vertex the cut creates still
// has a bit-identical twin in a sibling, because every crossing comes from
// the same edge on both sides of its line.
#[test]
fn cut_vertices_agree_bit_for_bit_on_a_slanted_face_cut_into_middle_pieces() {
    let quad: &[[f32; 2]] = &[
        [0.37, 0.11],
        [5.93, 0.83],
        [8.71, 3.29],
        [8.13, 8.97],
        [2.41, 8.53],
        [0.19, 5.07],
    ];
    let mut geometry = floor_faces(&[quad]);
    let originals: Vec<[u32; 3]> = geometry
        .geometry
        .vertices
        .iter()
        .map(|v| v.position.map(f32::to_bits))
        .collect();
    let mut charts = plan_charts(&geometry, 0.05, &[]).unwrap();
    let cuts = plan_face_cuts(&charts, 16);
    assert!(
        cuts[0].u_lines.len() >= 4 && cuts[0].v_lines.len() >= 4,
        "middle pieces on both axes: {cuts:?}"
    );
    let cut = apply_face_cuts(&mut geometry, &mut charts, &cuts);

    let siblings: Vec<Vec<u32>> = cut.face_remap[0]
        .clone()
        .map(|face| face_vertex_indices(&geometry, face))
        .collect();
    let bits = |index: u32| {
        let v = &geometry.geometry.vertices[index as usize];
        (v.position.map(f32::to_bits), v.uv.map(f32::to_bits))
    };
    let mut cut_vertices = 0;
    for (i, own) in siblings.iter().enumerate() {
        for &index in own {
            if originals.contains(&bits(index).0) {
                continue;
            }
            let twinned = siblings
                .iter()
                .enumerate()
                .any(|(j, other)| j != i && other.iter().any(|&o| bits(o) == bits(index)));
            assert!(
                twinned,
                "cut vertex {:?} has no bit-identical twin",
                geometry.geometry.vertices[index as usize].position
            );
            cut_vertices += 1;
        }
    }
    assert!(cut_vertices > 0);
    let total: f32 = cut.face_remap[0]
        .clone()
        .map(|face| face_area(&geometry, face))
        .sum();
    let expected = 0.5
        * quad
            .iter()
            .zip(quad.iter().cycle().skip(1))
            .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
            .sum::<f32>()
            .abs();
    assert!((total - expected).abs() < 1.0e-3, "{total} vs {expected}");
}

// The planned area sums the windows in closed form: it equals the
// window-by-window sum.
#[test]
fn planned_area_equals_the_sum_over_every_window() {
    let (_, _, _, cuts) = cut_square();
    let geometry = floor_faces(&[
        &square(-5.0, -5.0, 1.0),
        &square(0.0, 0.0, 9.0),
        &square(20.0, 20.0, 1.0),
    ]);
    let charts = plan_charts(&geometry, 0.1, &[]).unwrap();
    let by_window: u64 = [0, 2]
        .iter()
        .map(|&face| u64::from(charts[face].width_texels * charts[face].height_texels))
        .sum::<u64>()
        + cuts[0]
            .windows()
            .map(|([u, v], _)| u64::from((u.len() as u32 + PADDING) * (v.len() as u32 + PADDING)))
            .sum::<u64>();
    assert_eq!(planned_chart_area(&charts, &cuts), by_window);
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

/// A 9 m floor in cell 0 under a 2 m occluder straddling its cut lines, lit
/// by a soft area light whose penumbra and falloff cross both cuts.
fn penumbra_floor() -> (GeometryResult, Vec<crate::map_data::MapLight>) {
    let floor = square(0.0, 0.0, 9.0);
    let mut geometry = floor_faces(&[&floor, &square(3.5, 3.5, 2.0)]);
    // Raise the occluder 1 m and put both faces in cell 0.
    let occluder = geometry.face_index_ranges[1];
    let start = occluder.index_offset as usize;
    let occluder_vertices: Vec<u32> =
        geometry.geometry.indices[start..start + occluder.index_count as usize].to_vec();
    for index in occluder_vertices {
        geometry.geometry.vertices[index as usize].position[1] = 1.0;
    }
    for face in &mut geometry.geometry.faces {
        face.leaf_index = 0;
    }
    let light = crate::map_data::MapLight {
        origin: glam::DVec3::new(4.2, 3.0, 4.7),
        carrier: String::new(),
        light_type: crate::map_data::LightType::Point,
        intensity: 1.0,
        color: [1.0, 0.9, 0.8],
        falloff_model: crate::map_data::FalloffModel::Linear,
        falloff_range: 9.0,
        light_size: 1.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: true,
        is_animated: false,
        tags: Vec::new(),
        shadow_type: crate::map_data::ShadowType::StaticLightMap,
    };
    (geometry, vec![light])
}

/// For every pair of sub-charts of the cut floor, each parent-grid texel both
/// windows cover, as the two charts' interior texels.
/// A sub-chart's face and its window-local texel.
type WindowTexel = (usize, [u32; 2]);

fn overlap_twins(charts: &[Chart]) -> Vec<(WindowTexel, WindowTexel)> {
    let windowed: Vec<usize> = (0..charts.len())
        .filter(|&c| charts[c].window.is_some())
        .collect();
    let span = |chart: &Chart| {
        let window = chart.window.unwrap();
        let interior = [chart.width_texels - PADDING, chart.height_texels - PADDING];
        [
            window.origin[0]..window.origin[0] + interior[0],
            window.origin[1]..window.origin[1] + interior[1],
        ]
    };
    let mut twins = Vec::new();
    for (i, &a) in windowed.iter().enumerate() {
        for &b in &windowed[i + 1..] {
            let (sa, sb) = (span(&charts[a]), span(&charts[b]));
            let u = sa[0].start.max(sb[0].start)..sa[0].end.min(sb[0].end);
            let v = sa[1].start.max(sb[1].start)..sa[1].end.min(sb[1].end);
            for gy in v.clone() {
                for gx in u.clone() {
                    let local = |chart: &Chart| {
                        let origin = chart.window.unwrap().origin;
                        [gx - origin[0], gy - origin[1]]
                    };
                    twins.push(((a, local(&charts[a])), (b, local(&charts[b]))));
                }
            }
        }
    }
    twins
}

// Across every cut, overlap texels are bit-identical on both sides before
// encoding: irradiance, direction, and the raw soft visibility the
// shadowmask quantizes. A vertex on a cut lands on the same parent-grid
// texel from both sides, within vertex UV quantization (P20).
#[test]
fn overlap_texels_bake_bit_identical_across_every_cut() {
    use crate::bake_control::BakeControl;
    use crate::bvh_build::build_bvh;
    use crate::light_namespaces::StaticBakedLights;
    use crate::lightmap_bake::{BlockOrdering, DEFAULT_AREA_SAMPLE_COUNT, reference};
    use crate::lightmap_layer::{SharedAtlas, bake_light_layer_controlled};

    let (mut geometry, lights) = penumbra_floor();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let prepared = crate::lightmap_bake::prepare_atlas_within(
        &mut geometry,
        &static_lights,
        0.1,
        &[],
        BlockOrdering::by_cell_id(2),
        64,
        &BakeControl::unrestricted(),
    )
    .expect("the cut floor prepares");
    let charts = &prepared.charts;
    let twins = overlap_twins(charts);
    assert!(
        twins.len() > 100,
        "the cuts overlap: {} twin texels",
        twins.len()
    );
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let light_refs: Vec<_> = lights.iter().collect();

    // Irradiance and direction, per sub-chart.
    let baked: Vec<_> = charts
        .iter()
        .map(|chart| {
            reference::bake_face_chart(
                &bvh,
                &primitives,
                &geometry,
                &light_refs,
                chart,
                DEFAULT_AREA_SAMPLE_COUNT,
            )
        })
        .collect();
    let at = |chart: usize, [x, y]: [u32; 2]| {
        let atlas = &baked[chart];
        ((y + CHART_PADDING_TEXELS) * atlas.atlas_width + x + CHART_PADDING_TEXELS) as usize
    };
    let mut penumbra = 0;
    for &((a, ta), (b, tb)) in &twins {
        let (ia, ib) = (at(a, ta), at(b, tb));
        let irradiance = |chart: usize, i: usize| {
            baked[chart].irradiance[i * 4..i * 4 + 4]
                .iter()
                .map(|c| c.to_bits())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            irradiance(a, ia),
            irradiance(b, ib),
            "irradiance at {ta:?}/{tb:?}"
        );
        assert_eq!(
            baked[a].direction[ia].to_array().map(f32::to_bits),
            baked[b].direction[ib].to_array().map(f32::to_bits),
            "direction at {ta:?}/{tb:?}"
        );
        let red = baked[a].irradiance[ia * 4];
        if red > 0.0
            && red
                < 0.9
                    * baked[a]
                        .irradiance
                        .iter()
                        .step_by(4)
                        .cloned()
                        .fold(0.0, f32::max)
        {
            penumbra += 1;
        }
    }
    assert!(penumbra > 0, "a penumbra or falloff gradient crosses a cut");

    // Raw soft visibility, as the shipping per-light layer walk records it.
    let shared = SharedAtlas {
        charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let mut visibility = std::collections::HashMap::new();
    for layer in 0..prepared.layer_count {
        let partition = bake_light_layer_controlled(
            &lights[0],
            &shared,
            &bvh,
            &primitives,
            &geometry,
            layer,
            DEFAULT_AREA_SAMPLE_COUNT,
            &BakeControl::unrestricted(),
        );
        for texel in &partition.texels {
            visibility.insert((layer, texel.idx), texel.raw_visibility.to_bits());
        }
    }
    let atlas_key = |chart: usize, [x, y]: [u32; 2]| {
        let placement = prepared.placements[chart];
        let (ax, ay) = (
            placement.x + CHART_PADDING_TEXELS + x,
            placement.y + CHART_PADDING_TEXELS + y,
        );
        (placement.layer, ay * prepared.atlas_width + ax)
    };
    let mut compared = 0;
    for &((a, ta), (b, tb)) in &twins {
        let (va, vb) = (
            visibility.get(&atlas_key(a, ta)),
            visibility.get(&atlas_key(b, tb)),
        );
        assert_eq!(va, vb, "visibility at {ta:?}/{tb:?}");
        compared += usize::from(va.is_some());
    }
    assert!(compared > 0, "the light reaches the overlaps");

    // P20: a vertex on a cut maps to the same parent-grid position from both
    // of its sub-faces.
    let grid_position = |face: usize, vertex: &Vertex| {
        let chart = &charts[face];
        let block = &prepared.layout.blocks[usize::from(vertex.lightmap_block - 1)];
        let (local_x, local_y) = prepared
            .layout
            .local_placement(face, &prepared.placements[face]);
        let origin = chart.window.map_or([0, 0], |w| w.origin);
        let texel = |uv: u16, extent: u32, local: u32, origin: u32| {
            f32::from(uv) / 65535.0 * extent as f32 - local as f32 - CHART_PADDING_TEXELS as f32
                + origin as f32
        };
        [
            texel(vertex.lightmap_uv[0], block.width, local_x, origin[0]),
            texel(vertex.lightmap_uv[1], block.height, local_y, origin[1]),
            block.width.max(block.height) as f32 / 65535.0,
        ]
    };
    let mut seen: std::collections::HashMap<[u32; 3], (usize, [f32; 3])> =
        std::collections::HashMap::new();
    let mut shared_cut_vertices = 0;
    for (face, chart) in charts.iter().enumerate() {
        if chart.window.is_none() {
            continue;
        }
        for index in face_vertex_indices(&geometry, face) {
            let vertex = &geometry.geometry.vertices[index as usize];
            let position = grid_position(face, vertex);
            match seen.get(&vertex.position.map(f32::to_bits)) {
                Some(&(other, before)) if other != face => {
                    let tolerance = before[2] + position[2] + 1.0e-3;
                    assert!(
                        (before[0] - position[0]).abs() <= tolerance,
                        "{before:?} vs {position:?}"
                    );
                    assert!(
                        (before[1] - position[1]).abs() <= tolerance,
                        "{before:?} vs {position:?}"
                    );
                    shared_cut_vertices += 1;
                }
                _ => {
                    seen.insert(vertex.position.map(f32::to_bits), (face, position));
                }
            }
        }
    }
    assert!(shared_cut_vertices > 0);
}

// The encoded seam: BC6H encodes each sub-chart's 4×4 blocks on its own, so
// twin texels on a cut's bilinear footprint can decode differently. Both
// sides start bit-identical (above); the step a cut leaves must stay within
// the compression error BC6H already makes on ordinary texels of the same
// bake (measured: 0.085 at the cut against 0.113 elsewhere). Relative error
// misleads here: a penumbra block spans lit and shadowed texels, so its dark
// texels carry large relative but small absolute error.
#[test]
fn encoded_step_across_a_cut_stays_within_bc6h_noise() {
    use crate::bake_control::BakeControl;
    use crate::bc6h::{decode_bc6h_block_for_tests, f16_bits_to_f32};
    use crate::bvh_build::build_bvh;
    use crate::light_namespaces::StaticBakedLights;
    use crate::lightmap_bake::{
        BlockOrdering, DEFAULT_AREA_SAMPLE_COUNT, DIRECTION_TEXEL_SCALE, LightmapBakeCtx,
        LightmapConfig, bake_prepared_lightmap_controlled,
    };

    let (mut geometry, lights) = penumbra_floor();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let prepared = crate::lightmap_bake::prepare_atlas_within(
        &mut geometry,
        &static_lights,
        0.1,
        &[],
        BlockOrdering::by_cell_id(2),
        64,
        &BakeControl::unrestricted(),
    )
    .expect("the cut floor prepares");
    let charts = prepared.charts.clone();
    let placements = prepared.placements.clone();
    let layout = prepared.layout.clone();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let output = bake_prepared_lightmap_controlled(
        &mut LightmapBakeCtx {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &mut geometry,
            lights: &static_lights,
            scale_regions: &[],
        },
        &LightmapConfig {
            lightmap_density: 0.1,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: false,
        },
        prepared,
        &BakeControl::unrestricted(),
    )
    .expect("the cut floor bakes");

    let decoded: Vec<Vec<[f32; 3]>> = output
        .section
        .blocks
        .iter()
        .map(|block| {
            let (w, h) = (usize::from(block.width), usize::from(block.height));
            let mut texels = vec![[0.0; 3]; w * h];
            for (bi, encoded) in block.irradiance.chunks_exact(16).enumerate() {
                let (bx, by) = ((bi % (w / 4)) * 4, (bi / (w / 4)) * 4);
                let block = decode_bc6h_block_for_tests(encoded.try_into().unwrap());
                for (k, texel) in block.iter().enumerate() {
                    texels[(by + k / 4) * w + bx + k % 4] = texel.map(f16_bits_to_f32);
                }
            }
            texels
        })
        .collect();
    let sample = |face: usize, [x, y]: [u32; 2]| {
        let block = layout.chart_blocks[face] as usize;
        let (local_x, local_y) = layout.local_placement(face, &placements[face]);
        let width = u32::from(output.section.blocks[block].width);
        decoded[block][((local_y + CHART_PADDING_TEXELS + y) * width
            + local_x
            + CHART_PADDING_TEXELS
            + x) as usize]
    };

    // Twin texels a fragment on either side of a cut samples: the grid
    // texels just before and at each cut line.
    let cut_lines: Vec<u32> = charts
        .iter()
        .filter_map(|chart| chart.window)
        .flat_map(|window| [window.origin[0], window.origin[1]])
        .filter(|&origin| origin > 0)
        .map(|origin| origin + CUT_OVERLAP_TEXELS)
        .collect();
    let on_footprint = |grid: u32| cut_lines.iter().any(|&c| grid + 1 == c || grid == c);
    // BC6H's own absolute error on every interior texel, against the exact
    // pre-encode bake: the noise floor a cut must not exceed.
    let light_refs: Vec<_> = lights.iter().collect();
    let mut noise = 0.0f32;
    for (face, chart) in charts.iter().enumerate() {
        let exact = super::super::reference::bake_face_chart(
            &bvh,
            &primitives,
            &geometry,
            &light_refs,
            chart,
            DEFAULT_AREA_SAMPLE_COUNT,
        );
        for y in 0..chart.height_texels - PADDING {
            for x in 0..chart.width_texels - PADDING {
                let i = ((y + CHART_PADDING_TEXELS) * exact.atlas_width + x + CHART_PADDING_TEXELS)
                    as usize;
                let got = sample(face, [x, y]);
                for (c, value) in got.iter().enumerate() {
                    noise = noise.max((value - exact.irradiance[i * 4 + c]).abs());
                }
            }
        }
    }
    let (mut worst, mut worst_abs, mut compared) = (0.0f32, 0.0f32, 0usize);
    for ((a, ta), (b, tb)) in overlap_twins(&charts) {
        let origin = charts[a].window.unwrap().origin;
        let grid = [ta[0] + origin[0], ta[1] + origin[1]];
        if !on_footprint(grid[0]) && !on_footprint(grid[1]) {
            continue;
        }
        let (sa, sb) = (sample(a, ta), sample(b, tb));
        for c in 0..3 {
            let step = (sa[c] - sb[c]).abs();
            worst_abs = worst_abs.max(step);
            worst = worst.max(step / sa[c].max(sb[c]).max(0.01));
        }
        compared += 1;
    }
    println!(
        "encoded seam: {compared} twin texels on cut footprints, worst step {worst_abs:.4} ({worst:.4} relative); BC6H error elsewhere up to {noise:.4}"
    );
    assert!(compared > 0);
    assert!(
        worst_abs <= noise,
        "encoded step {worst_abs} at a cut past BC6H's error elsewhere, {noise}"
    );
}
