// Fluid surface packing, culling and slot tests.
// See: context/lib/testing_guide.md

use super::*;
use glam::IVec2;
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
    let mesh = pack_fluid_surfaces(&[volume(vec![face.clone()])], &[], 1, None);
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
        None,
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
    let mesh = pack_fluid_surfaces(&[volume(vec![face])], &[[64, 32]], 1, None);
    assert_eq!(mesh.vertices.len(), 5 + 1);
    let centroid = mesh.vertices[5];
    assert_eq!(centroid.position, [0.4, 1.0, 0.45]);
    assert_eq!(centroid.uv, [0.4, 0.45]);
}

#[test]
fn packing_takes_the_outward_normal_from_ccw_winding() {
    // CCW seen from +Y: the pool's top face points up.
    let up = pack_fluid_surfaces(&[volume(vec![top_face(0, [0.0; 3], 1.0)])], &[], 1, None);
    for v in &up.vertices {
        assert_eq!(v.normal, [0.0, 1.0, 0.0]);
    }

    // The same polygon wound the other way is the bottom of a floating block.
    let mut reversed = top_face(0, [0.0; 3], 1.0);
    reversed.vertices.reverse();
    let down = pack_fluid_surfaces(&[volume(vec![reversed])], &[], 1, None);
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
        None,
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
        None,
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
        None,
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
    let mesh = pack_fluid_surfaces(&volumes, &[], 1, None);
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

    assert!(pack_fluid_surfaces(&[], &[], 1, None).indices.is_empty());
}

fn opacity_all(opacity: f32) -> impl Fn(usize) -> Option<FluidSurfaceLook> {
    move |_| Some(FluidSurfaceLook::surface_only(opacity))
}

fn draw(volume_index: u32, look: FluidSurfaceLook) -> FluidSurfaceDraw {
    FluidSurfaceDraw {
        volume_index,
        params: look.pack(),
    }
}

fn clear_draw(volume_index: u32, opacity: f32) -> FluidSurfaceDraw {
    draw(volume_index, FluidSurfaceLook::surface_only(opacity))
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
    assert!(draws.iter().all(|draw| draw.params.opacity() == 0.6));
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
            2 => Some(FluidSurfaceLook::surface_only(0.0)),
            _ => Some(FluidSurfaceLook::surface_only(1.0)),
        },
    );
    assert!(draws.is_empty());

    // Only opaque-enough, present fluids draw; opacity clamps to 1.
    let draws = collector.collect(
        &volumes,
        &VisibleCells::DrawAll,
        cell_bounds,
        Vec3::ZERO,
        |index| (index == 2).then_some(FluidSurfaceLook::surface_only(4.0)),
    );
    assert_eq!(draws, [clear_draw(2, 1.0)]);
    assert_eq!(draws[0].params.opacity(), 1.0);
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

    let mesh = pack_fluid_surfaces(&[], &[], 1, None);
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
fn slots_upload_only_changed_rows_and_skip_volumes_without_surfaces() {
    let volumes = two_pools();
    let mesh = pack_fluid_surfaces(&volumes, &[], 1, None);
    let mut slots = FluidSurfaceSlots::new(&mesh.volumes);
    assert_eq!(slots.slot_count(), 2);
    assert_eq!(slots.window_count(), 1);

    let draws = [
        clear_draw(2, 0.5),
        // Dry and out-of-range volumes have no slot.
        clear_draw(1, 0.5),
        clear_draw(99, 0.5),
        clear_draw(0, 0.25),
    ];
    slots.set_draws(&draws);
    assert_eq!(slots.frame_slots(), [1, 0]);
    let (range, rows) = slots.take_dirty().expect("first draw uploads");
    assert_eq!(range, 0..2);
    assert_eq!(rows[0].surface, [0.25, 0.0, 0.0, 0.0]);
    assert_eq!(rows[1].surface, [0.5, 0.0, 0.0, 0.0]);

    // Same opacities next frame: nothing to upload.
    slots.set_draws(&draws);
    assert!(slots.take_dirty().is_none());
    assert!(slots.has_draws());

    // One fluid's opacity changes (a manifest reload): only its slot uploads.
    slots.set_draws(&[clear_draw(2, 0.75)]);
    let (range, rows) = slots.take_dirty().expect("changed slot uploads");
    assert_eq!(range, 1..2);
    assert_eq!(rows[0].surface, [0.75, 0.0, 0.0, 0.0]);

    slots.set_draws(&[]);
    assert!(!slots.has_draws());
}

/// Dev water by the brief's dev values.
fn water_look() -> FluidSurfaceLook {
    FluidSurfaceLook {
        surface_opacity: 0.75,
        fog_distance: Some(1.0),
        fog_color: [0.16, 0.22, 0.19],
        wave_period: Some(63.0),
        wave_amplitude: 8.0,
        wave_length: [64.0, 80.0],
        surface_reflectivity: 1.0,
        has_bound_face: false,
    }
}

fn sludge_look() -> FluidSurfaceLook {
    FluidSurfaceLook {
        fog_color: [0.2, 0.3, 0.1],
        ..FluidSurfaceLook::surface_only(0.9)
    }
}

#[test]
fn packing_lays_out_opacity_fog_and_wave_rows_with_spare_floats() {
    let row = water_look().pack();
    assert_eq!(row.surface, [0.75, 3.0, 0.0, 0.0]);
    let albedo = [0.16 * 0.25, 0.22 * 0.25, 0.19 * 0.25];
    for (packed, expected) in row.fog_albedo.iter().zip(albedo) {
        assert!((packed - expected).abs() < 1.0e-7);
    }
    assert_eq!(FLUID_FOG_COLOR_SCALE, 0.25);
    assert_eq!(row.wave, [1.0 / 63.0, 8.0, 64.0, 80.0]);
    assert_eq!(
        row.wave(),
        motion::FluidWaveParams {
            inv_period: 1.0 / 63.0,
            amplitude: 8.0,
            length_min: 64.0,
            length_max: 80.0,
        }
    );

    // Reflectivity rides `surface.z`, packed only for a volume with a bound
    // face; `surface.w` and `fog_albedo.w` stay spare for screen-space
    // reflections whatever the look.
    let bound = |look: FluidSurfaceLook| FluidSurfaceLook {
        has_bound_face: true,
        ..look
    };
    for (look, reflectivity) in [
        (water_look(), 0.0),
        (sludge_look(), 0.0),
        (FluidSurfaceLook::surface_only(1.0), 0.0),
        (bound(water_look()), 1.0),
        (bound(sludge_look()), 1.0),
        (
            FluidSurfaceLook {
                surface_reflectivity: 10.0,
                ..bound(FluidSurfaceLook::surface_only(1.0))
            },
            10.0,
        ),
    ] {
        let row = look.pack();
        assert_eq!(row.surface[2], reflectivity, "{look:?}");
        assert_eq!(row.reflectivity(), reflectivity);
        assert_eq!([row.surface[3], row.fog_albedo[3]], [0.0; 2], "{look:?}");
    }
    // Out-of-range and non-finite reflectivities pack inside 0..=50.
    for (declared, packed) in [
        (60.0, 50.0),
        (-1.0, 0.0),
        (f32::NAN, 0.0),
        (f32::INFINITY, 0.0),
    ] {
        let look = FluidSurfaceLook {
            surface_reflectivity: declared,
            ..bound(water_look())
        };
        assert_eq!(
            look.pack().reflectivity(),
            packed,
            "reflectivity {declared}"
        );
    }
    assert_eq!(std::mem::size_of::<FluidParamsRow>(), FLUID_PARAMS_STRIDE);
}

// Pin P3: a declared amplitude with no period, or amplitude 0, packs no wave,
// so every cell's wave offset is 0 at every time; fog omitted packs extinction 0.
#[test]
fn packing_zeroes_the_wave_without_a_period_or_amplitude_and_the_fog_without_a_distance() {
    let no_period = FluidSurfaceLook {
        wave_period: None,
        ..water_look()
    }
    .pack();
    assert_eq!(no_period.wave[1], 0.0);
    let flat = FluidSurfaceLook {
        wave_amplitude: 0.0,
        ..water_look()
    }
    .pack();
    assert_eq!(flat.wave[1], 0.0);
    for row in [no_period, flat] {
        for time in [0.0, 17.5, 86_399.0] {
            for cell in [IVec2::new(0, 0), IVec2::new(-37, 91), IVec2::new(500, -2)] {
                assert_eq!(motion::wave_offset(cell, time, row.wave()), IVec2::ZERO);
            }
        }
    }

    let clear = FluidSurfaceLook {
        fog_distance: None,
        ..water_look()
    }
    .pack();
    assert_eq!(clear.fog_extinction(), 0.0);
    for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let row = FluidSurfaceLook {
            fog_distance: Some(bad),
            ..water_look()
        }
        .pack();
        assert_eq!(row.fog_extinction(), 0.0, "fogDistance {bad} packs no fog");
    }
}

// The smallest positive `fogDistance` validates, but `3 / distance` overflows
// to +inf there, and the shader's `exp(-inf * 0)` on a zero path is NaN. The
// packed extinction stays finite, on a zero path still shows the scene, and
// times the longest path the shader can solve (`FLUID_FOG_PATH_MAX` camera
// distances, a far-clip corner's distance generously rounded up) still
// does not overflow, so the transmittance there is exactly 0.
#[test]
fn packing_keeps_the_extinction_finite_for_the_smallest_positive_fog_distance() {
    let longest_path = fog::FLUID_FOG_PATH_MAX * 8_192.0;
    for distance in [f32::from_bits(1), f32::MIN_POSITIVE, 1.0e-38] {
        let row = FluidSurfaceLook {
            fog_distance: Some(distance),
            ..water_look()
        }
        .pack();
        let extinction = row.fog_extinction();
        assert!(
            extinction.is_finite() && extinction > 0.0,
            "fogDistance {distance:e} packs extinction {extinction}"
        );
        assert_eq!(fog::fog_transmittance(extinction, 0.0), 1.0);
        assert_eq!(fog::fog_transmittance(extinction, 1.0e-3), 0.0);
        assert!(
            (extinction * longest_path).is_finite(),
            "fogDistance {distance:e}: {extinction} × {longest_path} overflows"
        );
        assert_eq!(fog::fog_transmittance(extinction, longest_path), 0.0);
    }
}

// A period so small its reciprocal overflows to +inf would turn the shader's
// phase split to NaN; it packs no wave, as an omitted period does.
#[test]
fn packing_drops_a_wave_whose_period_reciprocal_overflows() {
    for period in [f32::from_bits(1), 1.0e-39] {
        assert!(!(1.0 / period).is_finite());
        let row = FluidSurfaceLook {
            wave_period: Some(period),
            ..water_look()
        }
        .pack();
        assert_eq!(row.wave, [0.0; 4], "wavePeriod {period:e} packs no wave");
        for time in [0.0, 17.5, 86_399.0] {
            assert_eq!(
                motion::wave_offset(IVec2::new(-37, 91), time, row.wave()),
                IVec2::ZERO
            );
        }
    }
    // The smallest normal period's reciprocal is finite and still packs.
    let tiny = FluidSurfaceLook {
        wave_period: Some(f32::MIN_POSITIVE),
        ..water_look()
    }
    .pack();
    assert!(tiny.wave[0].is_finite() && tiny.wave[1] == 8.0);
}

// Pin P6: a reload that changes `wavePeriod` 63 → 20 draws, on its first
// frame, exactly a fresh evaluation of the wave at period 20 and that frame's
// time: the row carries no phase, so nothing eases across the change.
#[test]
fn a_reloaded_wave_period_takes_effect_as_a_fresh_evaluation_on_the_next_row() {
    let time = 4_321.25;
    let before = water_look().pack();
    let after = FluidSurfaceLook {
        wave_period: Some(20.0),
        ..water_look()
    }
    .pack();
    assert_ne!(before, after);
    let fresh = motion::FluidWaveParams {
        inv_period: 1.0 / 20.0,
        amplitude: 8.0,
        length_min: 64.0,
        length_max: 80.0,
    };
    for x in -40..40 {
        let cell = IVec2::new(x * 7, x * 13 - 5);
        assert_eq!(
            motion::wave_offset(cell, time, after.wave()),
            motion::wave_offset(cell, time, fresh)
        );
    }
}

// Pin P2: a reload that changes only one fluid's fog, wave or (with `fogColor`
// omitted) `viewTint` rgb re-uploads exactly that fluid's slots on the next
// frame; a reload that changes nothing uploads nothing.
#[test]
fn a_reload_reuploads_exactly_the_changed_fluids_slots_and_nothing_when_unchanged() {
    // Water in volumes 0 and 2, sludge in volume 3.
    let volumes = vec![
        volume(vec![top_face(0, [0.0; 3], 0.0)]),
        dry_volume(),
        volume(vec![top_face(0, [10.0, 0.0, 0.0], 0.0)]),
        volume(vec![top_face(0, [20.0, 0.0, 0.0], 0.0)]),
    ];
    let mesh = pack_fluid_surfaces(&volumes, &[], 1, None);
    let mut slots = FluidSurfaceSlots::new(&mesh.volumes);
    let frame = |water: FluidSurfaceLook| [draw(3, sludge_look()), draw(2, water), draw(0, water)];
    slots.set_draws(&frame(water_look()));
    assert_eq!(slots.take_dirty().map(|(range, _)| range), Some(0..3));

    // A reload that changes nothing.
    slots.set_draws(&frame(water_look()));
    assert!(
        slots.take_dirty().is_none(),
        "an unchanged reload uploads nothing"
    );

    let changes = [
        FluidSurfaceLook {
            fog_distance: Some(3.0),
            ..water_look()
        },
        // A `viewTint` rgb change with `fogColor` omitted reaches the look as
        // a new resolved fog colour.
        FluidSurfaceLook {
            fog_color: [0.1, 0.3, 0.4],
            ..water_look()
        },
        FluidSurfaceLook {
            wave_period: Some(20.0),
            ..water_look()
        },
        FluidSurfaceLook {
            wave_amplitude: 6.0,
            ..water_look()
        },
        FluidSurfaceLook {
            wave_length: [48.0, 80.0],
            ..water_look()
        },
    ];
    for changed in changes {
        slots.set_draws(&frame(water_look()));
        slots.take_dirty();
        slots.set_draws(&frame(changed));
        let (range, rows) = slots.take_dirty().expect("the changed fluid uploads");
        // Water's slots are 0 and 1; sludge's (2) stays clean.
        assert_eq!(range, 0..2, "{changed:?}");
        assert!(rows.iter().all(|row| *row == changed.pack()));
        slots.set_draws(&frame(changed));
        assert!(slots.take_dirty().is_none());
    }
}

// Pin P12 (reflectivity): a staged reload that changes only one fluid's
// `surfaceReflectivity` re-uploads exactly that fluid's slots on the next
// frame, and an unchanged re-collect uploads nothing. A volume with no bound
// face packs 0 either way, so its reflectivity change uploads nothing.
#[test]
fn a_reload_changing_only_reflectivity_reuploads_that_fluids_slots_and_nothing_when_unchanged() {
    let volumes = vec![
        volume(vec![top_face(0, [0.0; 3], 0.0)]),
        volume(vec![top_face(0, [10.0, 0.0, 0.0], 0.0)]),
        volume(vec![top_face(0, [20.0, 0.0, 0.0], 0.0)]),
    ];
    let mesh = pack_fluid_surfaces(&volumes, &[], 1, None);
    let mut slots = FluidSurfaceSlots::new(&mesh.volumes);
    let bound = FluidSurfaceLook {
        has_bound_face: true,
        ..water_look()
    };
    // Water in volumes 0 and 2 (bound), sludge in volume 1 (no bound face).
    let frame = |water: FluidSurfaceLook, sludge: FluidSurfaceLook| {
        [draw(1, sludge), draw(2, water), draw(0, water)]
    };
    slots.set_draws(&frame(bound, sludge_look()));
    assert_eq!(slots.take_dirty().map(|(range, _)| range), Some(0..3));
    slots.set_draws(&frame(bound, sludge_look()));
    assert!(
        slots.take_dirty().is_none(),
        "an unchanged re-collect uploads nothing"
    );

    for reflectivity in [2.0, 0.0, 50.0, 1.0] {
        let changed = FluidSurfaceLook {
            surface_reflectivity: reflectivity,
            ..bound
        };
        slots.set_draws(&frame(changed, sludge_look()));
        let (range, rows) = slots.take_dirty().expect("the changed fluid uploads");
        assert_eq!(range, 0..3, "water's slots 0 and 2 and the one between");
        assert_eq!(rows[0], changed.pack());
        assert_eq!(rows[2], changed.pack());
        assert_eq!(rows[0].reflectivity(), reflectivity);
        assert_eq!(rows[1], sludge_look().pack(), "sludge's row stays current");
        slots.set_draws(&frame(changed, sludge_look()));
        assert!(
            slots.take_dirty().is_none(),
            "{reflectivity}: unchanged next frame"
        );
    }
    let unbound_change = FluidSurfaceLook {
        surface_reflectivity: 7.0,
        ..sludge_look()
    };
    slots.set_draws(&frame(bound, unbound_change));
    assert!(
        slots.take_dirty().is_none(),
        "no bound face: reflectivity packs 0 and uploads nothing"
    );
}

// Pin P24: a zero-opacity, fog-free fluid with bound faces leaves the draw
// set on the frame a reload commits reflectivity 1 → 0, and rejoins it on the
// frame after the reverse reload.
#[test]
fn a_reload_flips_a_zero_opacity_fluids_draw_with_its_reflectivity() {
    let volumes = two_pools();
    let mut collector = FluidSurfaceRenderCollector::new();
    let look = |reflectivity: f32| FluidSurfaceLook {
        surface_reflectivity: reflectivity,
        has_bound_face: true,
        ..FluidSurfaceLook::surface_only(0.0)
    };
    let mut collect = |reflectivity: f32| {
        volume_indices(collector.collect(
            &volumes,
            &VisibleCells::DrawAll,
            cell_bounds,
            Vec3::ZERO,
            |_| Some(look(reflectivity)),
        ))
    };
    let reflecting = collect(1.0);
    assert!(!reflecting.is_empty());
    assert!(
        collect(0.0).is_empty(),
        "frame N after 1 → 0 collects no draw"
    );
    assert_eq!(collect(1.0), reflecting, "the reverse reload draws again");
}

// The upload is one contiguous range, so a change to a fluid whose slots
// straddle another's rewrites the one between: water at slots 0 and 2, sludge
// at 1. The range runs from water's first changed slot to its last, and every
// row in it is the current row for its slot. Sludge's row goes up again with
// its identical bytes, never stale or mixed with water's.
#[test]
fn an_interleaved_change_uploads_one_range_whose_every_row_is_current() {
    // Slots follow wet volume order: water 0 → slot 0, sludge 1 → slot 1,
    // water 2 → slot 2.
    let volumes = vec![
        volume(vec![top_face(0, [0.0; 3], 0.0)]),
        volume(vec![top_face(0, [10.0, 0.0, 0.0], 0.0)]),
        volume(vec![top_face(0, [20.0, 0.0, 0.0], 0.0)]),
    ];
    let mesh = pack_fluid_surfaces(&volumes, &[], 1, None);
    let mut slots = FluidSurfaceSlots::new(&mesh.volumes);
    let frame = |water: FluidSurfaceLook| [draw(1, sludge_look()), draw(2, water), draw(0, water)];
    slots.set_draws(&frame(water_look()));
    assert_eq!(slots.take_dirty().map(|(range, _)| range), Some(0..3));
    slots.set_draws(&frame(water_look()));
    assert!(slots.take_dirty().is_none());

    let changed = FluidSurfaceLook {
        fog_distance: Some(3.0),
        ..water_look()
    };
    slots.set_draws(&frame(changed));
    let (range, rows) = slots.take_dirty().expect("the changed fluid uploads");
    assert_eq!(range, 0..3, "from water's first changed slot to its last");
    let current = [changed.pack(), sludge_look().pack(), changed.pack()];
    assert_eq!(rows, current);
    assert_eq!(
        bytemuck::cast_slice::<FluidParamsRow, u8>(&rows[1..2]),
        bytemuck::bytes_of(&sludge_look().pack()),
        "sludge's row goes up with its identical bytes"
    );
    slots.set_draws(&frame(changed));
    assert!(slots.take_dirty().is_none());
}

// A fogged fluid at surface opacity 0 draws its fog alone; with fog omitted it
// still draws nothing (rendering_pipeline.md §7.3a).
#[test]
fn collector_keeps_a_zero_opacity_fluid_only_when_it_has_fog() {
    let volumes = two_pools();
    let mut collector = FluidSurfaceRenderCollector::new();
    let fogged = FluidSurfaceLook {
        fog_distance: Some(1.0),
        fog_color: [0.1, 0.3, 0.4],
        ..FluidSurfaceLook::surface_only(0.0)
    };
    let draws = collector.collect(
        &volumes,
        &VisibleCells::DrawAll,
        cell_bounds,
        Vec3::ZERO,
        |_| Some(fogged),
    );
    assert_eq!(volume_indices(draws), [2, 0]);
    assert!(
        draws
            .iter()
            .all(|draw| draw.params.opacity() == 0.0 && draw.params.fog_extinction() == 3.0)
    );

    let draws = collector.collect(
        &volumes,
        &VisibleCells::DrawAll,
        cell_bounds,
        Vec3::ZERO,
        |_| {
            Some(FluidSurfaceLook {
                fog_distance: None,
                ..fogged
            })
        },
    );
    assert!(draws.is_empty());
}

// Pins P2 and P23 (zero-opacity draw set): a fog-free fluid at surface
// opacity 0 that reflects (reflectivity > 0) draws its reflection alone when
// its volume has a probe-bound face; with no bound face, or at reflectivity
// 0, it collects no draw, as before reflections.
#[test]
fn collector_keeps_a_zero_opacity_fog_free_fluid_only_when_a_bound_face_reflects() {
    let volumes = two_pools();
    let mut collector = FluidSurfaceRenderCollector::new();
    let clear = FluidSurfaceLook::surface_only(0.0);
    assert_eq!(clear.surface_reflectivity, 1.0);
    // Only volume 2 binds a probe.
    let draws = collector.collect(
        &volumes,
        &VisibleCells::DrawAll,
        cell_bounds,
        Vec3::ZERO,
        |volume| {
            Some(FluidSurfaceLook {
                has_bound_face: volume == 2,
                ..clear
            })
        },
    );
    assert_eq!(volume_indices(draws), [2]);
    assert_eq!(draws[0].params.opacity(), 0.0);
    assert_eq!(draws[0].params.fog_extinction(), 0.0);
    assert_eq!(draws[0].params.reflectivity(), 1.0);

    // P23: reflectivity > 0 with no bound face anywhere draws nothing.
    let draws = collector.collect(
        &volumes,
        &VisibleCells::DrawAll,
        cell_bounds,
        Vec3::ZERO,
        |_| Some(clear),
    );
    assert!(draws.is_empty(), "{draws:?}");

    // P2: a bound face at reflectivity 0 and no fog draws nothing.
    let draws = collector.collect(
        &volumes,
        &VisibleCells::DrawAll,
        cell_bounds,
        Vec3::ZERO,
        |_| {
            Some(FluidSurfaceLook {
                surface_reflectivity: 0.0,
                has_bound_face: true,
                ..clear
            })
        },
    );
    assert!(draws.is_empty(), "{draws:?}");
}

// Two fluid pipelines: a draw takes the reflecting shader variant only when
// its volume has a probe-bound face, its reflectivity is above 0, the
// frame's lighting mask holds every bit of `LightTermMask::ALL`, and the
// reflection tier is not Off; every other draw takes the reflection-free
// variant. Opacity and fog never choose it. A probe-bound volume at
// reflectivity 1 under the all-terms mask is reflection-free at Off and
// reflecting at low, medium and high.
#[test]
fn shader_variant_reflects_only_a_bound_face_above_zero_reflectivity_under_every_term() {
    use crate::fluid_surface::reflection_quality::FluidReflectionQuality;
    use crate::frame_uniforms::LightTermMask;
    let all = LightTermMask::ALL.bits();
    // Every isolating mask: each term off alone, and every term off.
    let mut masks: Vec<(u32, bool)> = vec![(all, true), (all | 0x80, true), (0, false)];
    for term in LightTermMask::ALL_TERMS {
        let mut mask = LightTermMask::ALL;
        mask.set_enabled(term, false);
        masks.push((mask.bits(), false));
    }
    assert!(masks.contains(&(0x13F, false)), "specular off isolates");

    for has_bound_face in [false, true] {
        for reflectivity in [0.0, 0.001, 1.0, 50.0] {
            for opacity in [0.0, 0.6] {
                for fog_distance in [None, Some(4.0)] {
                    let row = FluidSurfaceLook {
                        surface_reflectivity: reflectivity,
                        has_bound_face,
                        fog_distance,
                        ..FluidSurfaceLook::surface_only(opacity)
                    }
                    .pack();
                    for &(mask, every_term) in &masks {
                        for quality in FluidReflectionQuality::ALL {
                            let expected = if has_bound_face
                                && reflectivity > 0.0
                                && every_term
                                && quality != FluidReflectionQuality::Off
                            {
                                FluidShaderVariant::Reflecting
                            } else {
                                FluidShaderVariant::ReflectionFree
                            };
                            assert_eq!(
                                row.shader_variant(mask, quality),
                                expected,
                                "bound {has_bound_face}, reflectivity {reflectivity}, opacity \
                                 {opacity}, fog {fog_distance:?}, mask {mask:#x}, {quality:?}"
                            );
                        }
                    }
                }
            }
        }
    }
    let probe_bound = FluidSurfaceLook {
        surface_reflectivity: 1.0,
        has_bound_face: true,
        ..FluidSurfaceLook::surface_only(0.6)
    }
    .pack();
    assert_eq!(
        probe_bound.shader_variant(all, FluidReflectionQuality::Off),
        FluidShaderVariant::ReflectionFree
    );
    for quality in [
        FluidReflectionQuality::Low,
        FluidReflectionQuality::Medium,
        FluidReflectionQuality::High,
    ] {
        assert_eq!(
            probe_bound.shader_variant(all, quality),
            FluidShaderVariant::Reflecting,
            "{quality:?}"
        );
    }
    assert!(FluidShaderVariant::Reflecting.reflections());
    assert!(!FluidShaderVariant::ReflectionFree.reflections());
}

// The frame's draws carry their variants in the collector's far-to-near
// order, so the pass switches pipeline only where neighbours differ; an
// isolating mask, or the Off tier, turns every draw reflection-free, and no
// tier changes the draw order.
#[test]
fn slots_pair_each_frame_draw_with_its_shader_variant_in_draw_order() {
    use crate::fluid_surface::reflection_quality::FluidReflectionQuality;
    let volumes = vec![
        volume(vec![top_face(0, [0.0; 3], 0.0)]),
        volume(vec![top_face(0, [2.0, 0.0, 0.0], 0.0)]),
        volume(vec![top_face(0, [4.0, 0.0, 0.0], 0.0)]),
    ];
    let mesh = pack_fluid_surfaces(&volumes, &[[64, 32]], 1, None);
    let mut slots = FluidSurfaceSlots::new(&mesh.volumes);
    let reflecting = FluidSurfaceLook {
        has_bound_face: true,
        ..FluidSurfaceLook::surface_only(0.6)
    };
    let unbound = FluidSurfaceLook::surface_only(0.6);
    slots.set_draws(&[
        draw(2, unbound),
        draw(0, reflecting),
        draw(
            1,
            FluidSurfaceLook {
                surface_reflectivity: 0.0,
                ..reflecting
            },
        ),
    ]);
    let all = crate::frame_uniforms::LightTermMask::ALL.bits();
    let variants: Vec<_> = slots
        .frame_draws(all, FluidReflectionQuality::High)
        .collect();
    assert_eq!(
        variants,
        [
            (2, FluidShaderVariant::ReflectionFree),
            (0, FluidShaderVariant::Reflecting),
            (1, FluidShaderVariant::ReflectionFree),
        ]
    );
    for quality in FluidReflectionQuality::ALL {
        assert!(
            slots
                .frame_draws(0x13F, quality)
                .all(|(_, variant)| variant == FluidShaderVariant::ReflectionFree)
        );
        let tiered: Vec<_> = slots.frame_draws(all, quality).collect();
        let expected: Vec<_> = if quality.reflects() {
            variants.clone()
        } else {
            variants
                .iter()
                .map(|&(slot, _)| (slot, FluidShaderVariant::ReflectionFree))
                .collect()
        };
        assert_eq!(tiered, expected, "{quality:?}");
    }
}

// Steady-state collection reuses its buffers: later frames of the same view
// keep the first frame's draw allocation.
#[test]
fn collector_reuses_its_buffers_in_steady_state() {
    let volumes = two_pools();
    let visible = VisibleCells::Culled(vec![0, 1]);
    let mut collector = FluidSurfaceRenderCollector::new();
    let mut collect = || {
        collector
            .collect(&volumes, &visible, cell_bounds, Vec3::ZERO, |_| {
                Some(water_look())
            })
            .as_ptr()
    };
    let first = collect();
    for _ in 0..4 {
        assert_eq!(collect(), first);
    }
}

#[test]
fn slot_windows_address_1360_row_uniform_windows_at_256_byte_offsets() {
    assert_eq!(FLUID_PARAMS_STRIDE, 48);
    assert_eq!(FLUID_SLOTS_PER_WINDOW, 1360);
    assert_eq!(FLUID_PARAMS_WINDOW_BYTES, 65_280);
    assert_eq!(slot_window(0), (0, 0));
    assert_eq!(slot_window(1359), (0, 1359));
    assert_eq!(slot_window(1360), (1, 0));
    assert_eq!(slot_window(9000), (6, 840));
    // Pin P1: every window's dynamic offset is a multiple of 256 and fits the
    // 64 KiB D3D constant-buffer ceiling.
    for window in 0..8u64 {
        assert_eq!((window * FLUID_PARAMS_WINDOW_BYTES) % 256, 0);
    }
    const { assert!(FLUID_PARAMS_WINDOW_BYTES <= 65_536) };
    // A slot's flat byte offset is its window offset plus its element offset,
    // so one contiguous buffer holds every window.
    for slot in [1359, 1360, 9000] {
        let (window, element) = slot_window(slot);
        assert_eq!(
            window as u64 * FLUID_PARAMS_WINDOW_BYTES + element as u64 * FLUID_PARAMS_STRIDE as u64,
            slot as u64 * FLUID_PARAMS_STRIDE as u64
        );
    }

    let many: Vec<FluidSurfaceVolume> = (0..1361)
        .map(|i| FluidSurfaceVolume {
            volume_index: i,
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ZERO,
            material_ranges: Vec::new(),
        })
        .collect();
    assert_eq!(FluidSurfaceSlots::new(&many).window_count(), 2);
    assert_eq!(FluidSurfaceSlots::new(&many[..1360]).window_count(), 1);
}
