// Fluid surface packing, culling and slot tests.
// See: context/lib/testing_guide.md

use super::*;
use postretro_level_format::environment_volumes::EnvironmentSurfaceVertex;
use postretro_visibility::VisibleCells;

fn vertex(position: [f32; 3], uv: [f32; 2]) -> EnvironmentSurfaceVertex {
    EnvironmentSurfaceVertex { position, uv }
}

/// A unit square in the XZ plane at height `y`, CCW seen from +Y (outside of
/// a pool's top), with texel UVs spanning 64×32 texels.
fn top_face(texture_index: u32, origin: [f32; 3], y: f32) -> EnvironmentSurfaceFace {
    let [x, _, z] = origin;
    EnvironmentSurfaceFace {
        texture_index,
        vertices: vec![
            vertex([x, y, z], [0.0, 0.0]),
            vertex([x, y, z + 1.0], [0.0, 32.0]),
            vertex([x + 1.0, y, z + 1.0], [64.0, 32.0]),
            vertex([x + 1.0, y, z], [64.0, 0.0]),
        ],
    }
}

fn volume(faces: Vec<EnvironmentSurfaceFace>) -> EnvironmentVolumeRecord {
    EnvironmentVolumeRecord {
        source_entity: 0,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".to_string(),
        brushes: Vec::new(),
        surface_faces: faces,
    }
}

fn dry_volume() -> EnvironmentVolumeRecord {
    EnvironmentVolumeRecord {
        fluid: String::new(),
        ..volume(Vec::new())
    }
}

/// A level face through `points` (in-plane `[u, v]`, wound counter-clockwise
/// with v up), facing +Y.
fn level_face(points: &[[f32; 2]]) -> EnvironmentSurfaceFace {
    EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: points
            .iter()
            .map(|&[u, v]| vertex([u, 0.0, -v], [0.0, 0.0]))
            .collect(),
    }
}

/// Twice the triangle's area along `normal`: positive when it winds CCW
/// seen from the side `normal` points to.
fn signed_twice_area(a: DVec3, b: DVec3, c: DVec3, normal: DVec3) -> f64 {
    (b - a).cross(c - a).dot(normal)
}

/// Packs `face` alone and asserts the triangles tile it with no T-junction:
/// the mesh holds the face's vertices in order, plus at most one centroid
/// vertex after them; every triangle winds with the face and has area; every
/// face vertex is a corner; every face edge segment of non-zero length
/// borders exactly one triangle; no triangle edge passes through a face
/// vertex; and the triangles' areas sum to the face's. Returns the mesh.
fn assert_tiles_face_without_t_junctions(face: &EnvironmentSurfaceFace) -> FluidSurfaceMesh {
    let mesh = pack_fluid_surfaces(&[volume(vec![face.clone()])], &[], 1);
    let ring = face.vertices.len();
    let position = |i: u32| Vec3::from(mesh.vertices[i as usize].position).as_dvec3();
    let normal = Vec3::from(mesh.vertices[0].normal).as_dvec3();
    assert!(
        mesh.vertices.len() == ring || mesh.vertices.len() == ring + 1,
        "{} mesh vertices for a {ring}-vertex face",
        mesh.vertices.len()
    );
    for (packed, source) in mesh.vertices.iter().zip(&face.vertices) {
        assert_eq!(packed.position, source.position);
    }
    if let Some(centroid) = mesh.vertices.get(ring) {
        let mean = face
            .vertices
            .iter()
            .map(|v| Vec3::from(v.position).as_dvec3())
            .sum::<DVec3>()
            / ring as f64;
        let error = (Vec3::from(centroid.position).as_dvec3() - mean).length();
        assert!(
            error <= 1.0e-6 * (1.0 + mean.length()),
            "centroid off by {error}"
        );
        assert_eq!(centroid.normal, mesh.vertices[0].normal);
    }
    let triangles: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
    assert_eq!(mesh.indices.len(), triangles.len() * 3);

    for &[a, b, c] in &triangles {
        let twice = signed_twice_area(position(a), position(b), position(c), normal);
        assert!(twice > 0.0, "triangle {a} {b} {c} has twice-area {twice}");
    }
    for vertex in 0..mesh.vertices.len() as u32 {
        assert!(
            mesh.indices.contains(&vertex),
            "vertex {vertex} is no triangle corner"
        );
    }
    for edge in 0..ring as u32 {
        let next = (edge + 1) % ring as u32;
        let bordering = triangles
            .iter()
            .filter(|t| t.contains(&edge) && t.contains(&next))
            .count();
        let expected = usize::from(position(edge) != position(next));
        assert_eq!(bordering, expected, "edge {edge}-{next}");
    }
    for t in &triangles {
        for side in 0..3 {
            let (p, q) = (position(t[side]), position(t[(side + 1) % 3]));
            for vertex in (0..ring as u32).filter(|v| !t.contains(v)) {
                let w = position(vertex);
                let along = (w - p).dot(q - p) / (q - p).length_squared();
                let off_line = (w - p).cross(q - p).length() / (q - p).length();
                // Far below the 0.5 mm spacings the fixtures use, far above
                // their f32 rounding.
                assert!(
                    off_line > 1.0e-5 || along <= 0.0 || along >= 1.0,
                    "triangle edge {}-{} spans vertex {vertex}",
                    t[side],
                    t[(side + 1) % 3]
                );
            }
        }
    }
    let face_area: f64 = (1..ring as u32 - 1)
        .map(|i| signed_twice_area(position(0), position(i), position(i + 1), normal))
        .sum();
    let tiled_area: f64 = triangles
        .iter()
        .map(|&[a, b, c]| signed_twice_area(position(a), position(b), position(c), normal))
        .sum();
    assert!(
        (face_area - tiled_area).abs() <= face_area * 1.0e-6,
        "face {face_area}, triangles {tiled_area}"
    );
    mesh
}

#[test]
fn packing_triangulates_a_strictly_convex_face_into_vertex_count_minus_two_triangles() {
    let pentagon = EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: vec![
            vertex([0.0, 0.0, 0.0], [0.0, 0.0]),
            vertex([2.0, 0.0, 0.0], [0.0, 0.0]),
            vertex([3.0, 1.0, 0.0], [0.0, 0.0]),
            vertex([1.0, 2.0, 0.0], [0.0, 0.0]),
            vertex([-1.0, 1.0, 0.0], [0.0, 0.0]),
        ],
    };
    assert_tiles_face_without_t_junctions(&pentagon);

    let mesh = pack_fluid_surfaces(
        &[volume(vec![top_face(0, [0.0; 3], 1.0), pentagon])],
        &[[64, 32]],
        1,
    );
    // No added vertex: 2 triangles for the square, 3 for the pentagon.
    assert_eq!(mesh.vertices.len(), 4 + 5);
    // Without collinear vertices, each face is the fan from its first vertex,
    // index for index the triangulation before collinear handling.
    assert_eq!(mesh.indices, [0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7, 4, 7, 8]);
}

// Regression: a fan from vertex 0 collapsed beside collinear T-junction
// vertices and spanned an edge across them, so translucent seams cracked.
#[test]
fn packing_keeps_collinear_vertices_on_opposite_edges_as_triangle_corners() {
    let face = level_face(&[
        [0.0, 0.0],
        [32.0, 0.0],
        [48.0, 0.0],
        [128.0, 0.0],
        [128.0, 64.0],
        [48.0, 64.0],
        [32.0, 64.0],
        [0.0, 64.0],
    ]);
    let mesh = assert_tiles_face_without_t_junctions(&face);
    assert_eq!(mesh.vertices.len(), 8 + 1);
    assert_eq!(mesh.vertices[0].normal, [0.0, 1.0, 0.0]);
    // One triangle per edge segment, from the centroid.
    assert_eq!(mesh.indices.len(), 8 * 3);
    assert_eq!(mesh.volumes[0].material_ranges[0].index_count, 8 * 3);
}

#[test]
fn packing_keeps_a_single_collinear_vertex_as_a_triangle_corner() {
    // A triangle with one collinear vertex on its closing edge.
    let closing_edge = level_face(&[[0.0, 0.0], [64.0, 0.0], [32.0, 64.0], [16.0, 32.0]]);
    let mesh = assert_tiles_face_without_t_junctions(&closing_edge);
    assert_eq!(mesh.indices.len(), 4 * 3);

    // A quad with one collinear vertex beside the first corner.
    let first_edge = level_face(&[
        [0.0, 0.0],
        [20.0, 0.0],
        [64.0, 0.0],
        [64.0, 32.0],
        [0.0, 32.0],
    ]);
    let mesh = assert_tiles_face_without_t_junctions(&first_edge);
    assert_eq!(mesh.indices.len(), 5 * 3);
}

// The collinear tolerance is absolute, because the compiler's vertex spacing
// is absolute down to 0.1 mm: large faces whose vertices sit closer than any
// tolerance relative to face size still tile whole.
#[test]
fn packing_tiles_large_faces_whose_vertices_sit_half_a_millimeter_apart() {
    let corner_insert = level_face(&[
        [0.0, 0.0],
        [9.9995, 0.0],
        [10.0, 0.0],
        [10.0, 10.0],
        [0.0, 10.0],
    ]);
    let mesh = assert_tiles_face_without_t_junctions(&corner_insert);
    assert_eq!(mesh.vertices.len(), 5 + 1);

    // A corner chamfered by burial, with no insert.
    assert_tiles_face_without_t_junctions(&level_face(&[
        [0.0, 0.0],
        [9.9995, 0.0],
        [10.0, 0.0005],
        [10.0, 10.0],
        [0.0, 10.0],
    ]));

    // Two inserts 0.5 mm apart mid-edge.
    assert_tiles_face_without_t_junctions(&level_face(&[
        [0.0, 0.0],
        [5.0, 0.0],
        [5.0005, 0.0],
        [10.0, 0.0],
        [10.0, 10.0],
        [0.0, 10.0],
    ]));

    // The same on a triangle's oblique closing edge, each insert computed in
    // f64 and rounded to f32 as the compiler stores it.
    let apex = DVec2::new(5.0, 10.0);
    let toward_origin = -apex.normalize();
    let insert = |along: f64| (apex + toward_origin * along).as_vec2().to_array();
    let half = apex.length() * 0.5;
    assert_tiles_face_without_t_junctions(&level_face(&[
        [0.0, 0.0],
        [10.0, 0.0],
        [5.0, 10.0],
        insert(half),
        insert(half + 0.0005),
    ]));
}

#[test]
fn packing_tiles_a_thin_long_strip_with_and_without_an_insert() {
    assert_tiles_face_without_t_junctions(&level_face(&[
        [0.0, 0.0],
        [25.0, 0.0],
        [25.0, 0.002],
        [0.0, 0.002],
    ]));
    assert_tiles_face_without_t_junctions(&level_face(&[
        [0.0, 0.0],
        [12.5, 0.0],
        [25.0, 0.0],
        [25.0, 0.002],
        [0.0, 0.002],
    ]));
}

#[test]
fn packing_skips_only_the_zero_length_segment_of_a_duplicated_vertex() {
    let mid_ring = level_face(&[
        [0.0, 0.0],
        [10.0, 0.0],
        [10.0, 0.0],
        [10.0, 10.0],
        [0.0, 10.0],
    ]);
    let mesh = assert_tiles_face_without_t_junctions(&mid_ring);
    assert_eq!(mesh.indices.len(), 4 * 3);

    // The duplicate closes the ring.
    let closing = level_face(&[
        [0.0, 0.0],
        [10.0, 0.0],
        [10.0, 10.0],
        [0.0, 10.0],
        [0.0, 0.0],
    ]);
    let mesh = assert_tiles_face_without_t_junctions(&closing);
    assert_eq!(mesh.indices.len(), 4 * 3);
}

#[test]
fn packing_gives_an_added_centroid_the_faces_mean_uv() {
    // UV is affine in position, so the mean of the corners' UVs is exact.
    let mut face = top_face(0, [0.0; 3], 1.0);
    face.vertices
        .insert(1, vertex([0.0, 1.0, 0.25], [0.0, 8.0]));
    let mesh = pack_fluid_surfaces(&[volume(vec![face])], &[[64, 32]], 1);
    assert_eq!(mesh.vertices.len(), 5 + 1);
    let centroid = mesh.vertices[5];
    assert_eq!(centroid.position, [0.4, 1.0, 0.45]);
    assert_eq!(centroid.uv, [0.4, 0.45]);
}

#[test]
fn packing_takes_the_outward_normal_from_ccw_winding() {
    // CCW seen from +Y: the pool's top face points up.
    let up = pack_fluid_surfaces(&[volume(vec![top_face(0, [0.0; 3], 1.0)])], &[], 1);
    for v in &up.vertices {
        assert_eq!(v.normal, [0.0, 1.0, 0.0]);
    }

    // The same polygon wound the other way is the bottom of a floating block.
    let mut reversed = top_face(0, [0.0; 3], 1.0);
    reversed.vertices.reverse();
    let down = pack_fluid_surfaces(&[volume(vec![reversed])], &[], 1);
    for v in &down.vertices {
        assert_eq!(v.normal, [0.0, -1.0, 0.0]);
    }

    // An oblique face: CCW seen from +X+Y.
    let slope = EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: vec![
            vertex([0.0, 1.0, 0.0], [0.0, 0.0]),
            vertex([0.0, 1.0, 1.0], [0.0, 0.0]),
            vertex([1.0, 0.0, 1.0], [0.0, 0.0]),
            vertex([1.0, 0.0, 0.0], [0.0, 0.0]),
        ],
    };
    let n = face_normal(&slope).expect("non-degenerate");
    let expected = Vec3::new(1.0, 1.0, 0.0).normalize();
    assert!(n.abs_diff_eq(expected, 1.0e-6), "{n:?}");
}

#[test]
fn packing_normalizes_texel_uvs_by_the_face_texture_dimensions() {
    let mesh = pack_fluid_surfaces(
        &[volume(vec![top_face(1, [0.0; 3], 0.0)])],
        &[[16, 16], [64, 32]],
        2,
    );
    let uvs: Vec<[f32; 2]> = mesh.vertices.iter().map(|v| v.uv).collect();
    assert_eq!(uvs, [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]]);
}

#[test]
fn packing_leaves_uvs_raw_and_uses_the_placeholder_for_unknown_textures() {
    let mesh = pack_fluid_surfaces(
        &[volume(vec![
            top_face(7, [0.0; 3], 0.0),
            top_face(NO_TEXTURE, [2.0, 0.0, 0.0], 0.0),
        ])],
        &[[64, 32]],
        1,
    );
    // Unknown dimensions keep texel UVs, as world normalization does.
    assert_eq!(mesh.vertices[2].uv, [64.0, 32.0]);
    assert_eq!(mesh.vertices[6].uv, [64.0, 32.0]);
    // Both faces fall back to material 0 and merge into one range.
    assert_eq!(
        mesh.volumes[0].material_ranges,
        [FluidMaterialRange {
            material_index: 0,
            index_start: 0,
            index_count: 12,
        }]
    );
}

#[test]
fn packing_groups_each_volumes_faces_into_one_range_per_material() {
    let mesh = pack_fluid_surfaces(
        &[
            volume(vec![
                top_face(1, [0.0; 3], 0.0),
                top_face(0, [1.0, 0.0, 0.0], 0.0),
                top_face(1, [2.0, 0.0, 0.0], 0.0),
            ]),
            volume(vec![top_face(0, [5.0, 0.0, 0.0], 0.0)]),
        ],
        &[[64, 32], [64, 32]],
        2,
    );
    assert_eq!(mesh.volumes.len(), 2);
    assert_eq!(
        mesh.volumes[0].material_ranges,
        [
            FluidMaterialRange {
                material_index: 0,
                index_start: 0,
                index_count: 6,
            },
            FluidMaterialRange {
                material_index: 1,
                index_start: 6,
                index_count: 12,
            },
        ]
    );
    assert_eq!(
        mesh.volumes[1].material_ranges,
        [FluidMaterialRange {
            material_index: 0,
            index_start: 18,
            index_count: 6,
        }]
    );
    // Material order moved the second face's vertices first.
    assert_eq!(mesh.vertices[0].position, [1.0, 0.0, 0.0]);
}

#[test]
fn packing_gives_slots_only_to_volumes_with_drawable_faces_and_keeps_their_bounds() {
    let sliver = EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: vec![
            vertex([0.0, 0.0, 0.0], [0.0, 0.0]),
            vertex([1.0, 0.0, 0.0], [0.0, 0.0]),
            vertex([2.0, 0.0, 0.0], [0.0, 0.0]),
        ],
    };
    let volumes = [
        dry_volume(),
        volume(vec![sliver]),
        volume(vec![top_face(0, [3.0, 0.0, 4.0], 2.0)]),
    ];
    let mesh = pack_fluid_surfaces(&volumes, &[], 1);
    assert_eq!(mesh.volumes.len(), 1);
    let only = &mesh.volumes[0];
    assert_eq!(only.volume_index, 2);
    assert_eq!(only.bounds_min, Vec3::new(3.0, 2.0, 4.0));
    assert_eq!(only.bounds_max, Vec3::new(4.0, 2.0, 5.0));
    assert_eq!(
        surface_bounds(&volumes[2]),
        Some((only.bounds_min, only.bounds_max))
    );
    assert_eq!(surface_bounds(&volumes[1]), None);

    assert!(pack_fluid_surfaces(&[], &[], 1).indices.is_empty());
}

fn opacity_all(opacity: f32) -> impl Fn(usize) -> Option<f32> {
    move |_| Some(opacity)
}

/// Two pools on the X axis, at x 0..1 and x 10..11, plus a dry volume.
fn two_pools() -> Vec<EnvironmentVolumeRecord> {
    vec![
        volume(vec![top_face(0, [0.0; 3], 0.0)]),
        dry_volume(),
        volume(vec![top_face(0, [10.0, 0.0, 0.0], 0.0)]),
    ]
}

fn cell_bounds(cell: u32) -> Option<(Vec3, Vec3)> {
    match cell {
        // Covers the first pool only.
        0 => Some((Vec3::new(-1.0, -1.0, -1.0), Vec3::new(2.0, 2.0, 2.0))),
        // Covers the second pool only.
        1 => Some((Vec3::new(9.0, -1.0, -1.0), Vec3::new(12.0, 2.0, 2.0))),
        // Touches neither.
        2 => Some((Vec3::new(4.0, -1.0, -1.0), Vec3::new(6.0, 2.0, 2.0))),
        _ => None,
    }
}

fn volume_indices(draws: &[FluidSurfaceDraw]) -> Vec<u32> {
    draws.iter().map(|draw| draw.volume_index).collect()
}

#[test]
fn collector_draws_every_present_fluid_under_draw_all() {
    let volumes = two_pools();
    let mut collector = FluidSurfaceRenderCollector::new();
    let draws = collector.collect(
        &volumes,
        &VisibleCells::DrawAll,
        cell_bounds,
        Vec3::ZERO,
        opacity_all(0.6),
    );
    // Far to near from a camera at the first pool.
    assert_eq!(volume_indices(draws), [2, 0]);
    assert!(draws.iter().all(|draw| draw.surface_opacity == 0.6));
}

#[test]
fn collector_draws_only_volumes_touching_a_visible_cell() {
    let volumes = two_pools();
    let mut collector = FluidSurfaceRenderCollector::new();
    let culled = |cells: Vec<u32>| VisibleCells::Culled(cells);

    let draws = collector.collect(
        &volumes,
        &culled(vec![1]),
        cell_bounds,
        Vec3::ZERO,
        opacity_all(0.5),
    );
    assert_eq!(volume_indices(draws), [2]);

    let draws = collector.collect(
        &volumes,
        &culled(vec![2]),
        cell_bounds,
        Vec3::ZERO,
        opacity_all(0.5),
    );
    assert!(draws.is_empty());

    // An unknown cell id contributes no bounds.
    let draws = collector.collect(
        &volumes,
        &culled(vec![9, 0]),
        cell_bounds,
        Vec3::ZERO,
        opacity_all(0.5),
    );
    assert_eq!(volume_indices(draws), [0]);

    let draws = collector.collect(
        &volumes,
        &culled(Vec::new()),
        cell_bounds,
        Vec3::ZERO,
        opacity_all(0.5),
    );
    assert!(draws.is_empty());
}

#[test]
fn collector_skips_volumes_without_a_presentation_opacity() {
    let volumes = two_pools();
    let mut collector = FluidSurfaceRenderCollector::new();
    // The client lacks volume 0's fluid; volume 2's fluid is fully clear.
    let draws = collector.collect(
        &volumes,
        &VisibleCells::DrawAll,
        cell_bounds,
        Vec3::ZERO,
        |index| match index {
            0 => None,
            2 => Some(0.0),
            _ => Some(1.0),
        },
    );
    assert!(draws.is_empty());

    // Only opaque-enough, present fluids draw; opacity clamps to 1.
    let draws = collector.collect(
        &volumes,
        &VisibleCells::DrawAll,
        cell_bounds,
        Vec3::ZERO,
        |index| (index == 2).then_some(4.0),
    );
    assert_eq!(
        draws,
        [FluidSurfaceDraw {
            volume_index: 2,
            surface_opacity: 1.0,
        }]
    );
}

#[test]
fn collector_with_no_volumes_returns_no_draws_and_the_slots_gate_stays_closed() {
    let mut collector = FluidSurfaceRenderCollector::new();
    let draws = collector.collect(
        &[],
        &VisibleCells::DrawAll,
        cell_bounds,
        Vec3::ZERO,
        opacity_all(1.0),
    );
    assert!(draws.is_empty());

    let mesh = pack_fluid_surfaces(&[], &[], 1);
    let mut slots = FluidSurfaceSlots::new(&mesh.volumes);
    slots.set_draws(collector.draws());
    assert!(!slots.has_draws());
    assert!(slots.take_dirty().is_none());
}

#[test]
fn collector_rebuilds_bounds_when_the_level_changes() {
    let first = two_pools();
    let second = vec![volume(vec![top_face(0, [50.0, 0.0, 0.0], 0.0)])];
    let mut collector = FluidSurfaceRenderCollector::new();
    collector.collect(
        &first,
        &VisibleCells::DrawAll,
        cell_bounds,
        Vec3::ZERO,
        opacity_all(1.0),
    );
    let draws = collector.collect(
        &second,
        &VisibleCells::DrawAll,
        cell_bounds,
        Vec3::ZERO,
        opacity_all(1.0),
    );
    assert_eq!(volume_indices(draws), [0]);
    collector.clear();
    assert!(collector.draws().is_empty());
}

#[test]
fn slots_upload_only_changed_opacity_and_skip_volumes_without_surfaces() {
    let volumes = two_pools();
    let mesh = pack_fluid_surfaces(&volumes, &[], 1);
    let mut slots = FluidSurfaceSlots::new(&mesh.volumes);
    assert_eq!(slots.slot_count(), 2);
    assert_eq!(slots.window_count(), 1);

    let draws = [
        FluidSurfaceDraw {
            volume_index: 2,
            surface_opacity: 0.5,
        },
        // Dry and out-of-range volumes have no slot.
        FluidSurfaceDraw {
            volume_index: 1,
            surface_opacity: 0.5,
        },
        FluidSurfaceDraw {
            volume_index: 99,
            surface_opacity: 0.5,
        },
        FluidSurfaceDraw {
            volume_index: 0,
            surface_opacity: 0.25,
        },
    ];
    slots.set_draws(&draws);
    assert_eq!(slots.frame_slots(), [1, 0]);
    let (range, rows) = slots.take_dirty().expect("first draw uploads");
    assert_eq!(range, 0..2);
    assert_eq!(rows, [[0.25, 0.0, 0.0, 0.0], [0.5, 0.0, 0.0, 0.0]]);

    // Same opacities next frame: nothing to upload.
    slots.set_draws(&draws);
    assert!(slots.take_dirty().is_none());
    assert!(slots.has_draws());

    // One fluid's opacity changes (a manifest reload): only its slot uploads.
    slots.set_draws(&[FluidSurfaceDraw {
        volume_index: 2,
        surface_opacity: 0.75,
    }]);
    let (range, rows) = slots.take_dirty().expect("changed slot uploads");
    assert_eq!(range, 1..2);
    assert_eq!(rows, [[0.75, 0.0, 0.0, 0.0]]);

    slots.set_draws(&[]);
    assert!(!slots.has_draws());
}

#[test]
fn slot_windows_address_4096_vec4_uniform_windows() {
    assert_eq!(FLUID_PARAMS_WINDOW_BYTES, 65_536);
    assert_eq!(slot_window(0), (0, 0));
    assert_eq!(slot_window(4095), (0, 4095));
    assert_eq!(slot_window(4096), (1, 0));
    assert_eq!(slot_window(9000), (2, 808));
    // A slot's flat byte offset is its window offset plus its element offset,
    // so one contiguous buffer holds every window.
    let (window, element) = slot_window(9000);
    assert_eq!(
        window as u64 * FLUID_PARAMS_WINDOW_BYTES + element as u64 * FLUID_PARAMS_STRIDE as u64,
        9000 * FLUID_PARAMS_STRIDE as u64
    );

    let many: Vec<FluidSurfaceVolume> = (0..4097)
        .map(|i| FluidSurfaceVolume {
            volume_index: i,
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ZERO,
            material_ranges: Vec::new(),
        })
        .collect();
    assert_eq!(FluidSurfaceSlots::new(&many).window_count(), 2);
}
