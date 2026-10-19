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
/// the mesh holds exactly the face's vertices, in order; every triangle winds
/// with the face and has area; every face vertex is a corner, except one
/// repeating a ring neighbour's position, which may leave that twin to be;
/// every face edge segment of non-zero length borders exactly one triangle;
/// no triangle edge passes through a face vertex; and the triangles' areas
/// sum to the face's. Returns the mesh.
fn assert_tiles_face_without_t_junctions(face: &EnvironmentSurfaceFace) -> FluidSurfaceMesh {
    let mesh = pack_fluid_surfaces(&[volume(vec![face.clone()])], &[], 1, None);
    let ring = face.vertices.len();
    let position = |i: u32| Vec3::from(mesh.vertices[i as usize].position).as_dvec3();
    let normal = Vec3::from(mesh.vertices[0].normal).as_dvec3();
    assert_eq!(
        mesh.vertices.len(),
        ring,
        "{} mesh vertices for a {ring}-vertex face",
        mesh.vertices.len()
    );
    for (packed, source) in mesh.vertices.iter().zip(&face.vertices) {
        assert_eq!(packed.position, source.position);
    }
    let triangles: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
    assert_eq!(mesh.indices.len(), triangles.len() * 3);

    for &[a, b, c] in &triangles {
        let twice = signed_twice_area(position(a), position(b), position(c), normal);
        assert!(twice > 0.0, "triangle {a} {b} {c} has twice-area {twice}");
    }
    // The corner that stands for each face vertex: itself, or a twin in its
    // run of ring neighbours at the same position.
    let ring_step = |i: u32, step: u32| (i + step) % ring as u32;
    let stand_in = |vertex: u32| {
        for step in [1, ring as u32 - 1] {
            let mut i = vertex;
            for _ in 0..ring {
                if position(i) != position(vertex) {
                    break;
                }
                if mesh.indices.contains(&i) {
                    return i;
                }
                i = ring_step(i, step);
            }
        }
        panic!("vertex {vertex} is no triangle corner");
    };
    for vertex in 0..ring as u32 {
        stand_in(vertex);
    }
    // A zero-length segment borders nothing.
    for edge in (0..ring as u32).filter(|&i| position(i) != position(ring_step(i, 1))) {
        let next = ring_step(edge, 1);
        let (from, to) = (stand_in(edge), stand_in(next));
        let bordering = triangles
            .iter()
            .filter(|t| t.contains(&from) && t.contains(&to))
            .count();
        assert_eq!(bordering, 1, "edge {edge}-{next}");
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
    assert_eq!(mesh.vertices[0].normal, [0.0, 1.0, 0.0]);
    // Ear clipping over the face's own vertices: n - 2 triangles.
    assert_eq!(mesh.indices.len(), 6 * 3);
    assert_eq!(mesh.volumes[0].material_ranges[0].index_count, 6 * 3);
}

#[test]
fn packing_keeps_a_single_collinear_vertex_as_a_triangle_corner() {
    // A triangle with one collinear vertex on its closing edge.
    let closing_edge = level_face(&[[0.0, 0.0], [64.0, 0.0], [32.0, 64.0], [16.0, 32.0]]);
    let mesh = assert_tiles_face_without_t_junctions(&closing_edge);
    assert_eq!(mesh.indices.len(), 2 * 3);

    // A quad with one collinear vertex beside the first corner.
    let first_edge = level_face(&[
        [0.0, 0.0],
        [20.0, 0.0],
        [64.0, 0.0],
        [64.0, 32.0],
        [0.0, 32.0],
    ]);
    let mesh = assert_tiles_face_without_t_junctions(&first_edge);
    assert_eq!(mesh.indices.len(), 3 * 3);
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
    assert_tiles_face_without_t_junctions(&corner_insert);

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

// A vertex repeating its predecessor joins the ring once: its slot packs but
// no triangle reads it, its twin standing for the position, and the face
// still tiles whole.
#[test]
fn packing_joins_a_duplicated_vertex_to_its_twin() {
    let mid_ring = level_face(&[
        [0.0, 0.0],
        [10.0, 0.0],
        [10.0, 0.0],
        [10.0, 10.0],
        [0.0, 10.0],
    ]);
    let mesh = assert_tiles_face_without_t_junctions(&mid_ring);
    assert_eq!(mesh.indices.len(), 2 * 3);
    assert!(mesh.indices.contains(&1) && !mesh.indices.contains(&2));

    // The duplicate closes the ring.
    let closing = level_face(&[
        [0.0, 0.0],
        [10.0, 0.0],
        [10.0, 10.0],
        [0.0, 10.0],
        [0.0, 0.0],
    ]);
    let mesh = assert_tiles_face_without_t_junctions(&closing);
    assert_eq!(mesh.indices.len(), 2 * 3);
    assert!(mesh.indices.contains(&0) && !mesh.indices.contains(&4));
}

/// How far inside its edge every other insert of [`inserted_top`] sits on a
/// dented face, meters: within closure's 0.1 mm, so each such insert is a
/// reflex dent and the ear clip's containment index holds thousands.
const INSERT_DENT: f64 = 5.0e-5;

/// A 2 m × 11 m level top 100 m out at height 1.1, CCW seen from +Y, whose
/// x = 102 edge carries `inserts` vertices from z 7.9 down to 1: what closure
/// gives an ocean's edge beside many small pieces. Every other insert, from
/// the first, sits `dent` meters inside the edge, the rest on it. Positions
/// and texel UVs (one projection, about 39 texels per meter) are computed in
/// f64 and rounded to f32 separately, as the compiler stores them.
fn inserted_top(inserts: usize, dent: f64) -> EnvironmentSurfaceFace {
    let (x0, x1, z0, z1) = (100.0, 102.0, 1.0, 12.0);
    let mut points = vec![DVec2::new(x0, z0), DVec2::new(x0, z1), DVec2::new(x1, z1)];
    points.extend((0..inserts).map(|i| {
        let inside = if i % 2 == 0 { dent } else { 0.0 };
        DVec2::new(x1 - inside, 7.9 - 6.9 * (i as f64 + 0.5) / inserts as f64)
    }));
    points.push(DVec2::new(x1, z0));
    EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: points
            .iter()
            .map(|p| {
                vertex(
                    [p.x as f32, 1.1, p.y as f32],
                    (*p / 0.0254).as_vec2().to_array(),
                )
            })
            .collect(),
    }
}

/// Asserts the mesh of one up-facing face, its own vertices leading, tiles
/// it: every triangle winds with the face and has area, their areas sum to
/// the face's, every mesh vertex is a triangle corner, and each face segment
/// in `segments` (from ring vertex `i` to `i + 1`) is the edge of exactly one
/// triangle, so no triangle edge spans a vertex there. Linear in the mesh, so
/// it holds at thousands of vertices.
fn assert_tiles_up_facing_face(
    mesh: &FluidSurfaceMesh,
    face: &EnvironmentSurfaceFace,
    segments: std::ops::Range<u32>,
) {
    let position = |i: u32| Vec3::from(mesh.vertices[i as usize].position).as_dvec3();
    let mut corner = vec![false; mesh.vertices.len()];
    let mut edges = std::collections::HashMap::new();
    let mut tiled = 0.0;
    for &[a, b, c] in mesh.indices.as_chunks::<3>().0 {
        let twice = signed_twice_area(position(a), position(b), position(c), DVec3::Y);
        assert!(twice > 0.0, "triangle {a} {b} {c} has twice-area {twice}");
        tiled += twice;
        for (from, to) in [(a, b), (b, c), (c, a)] {
            corner[from as usize] = true;
            *edges.entry((from, to)).or_insert(0) += 1;
        }
    }
    for i in segments {
        assert_eq!(
            edges.get(&(i, i + 1)),
            Some(&1),
            "face segment {i}-{}",
            i + 1
        );
    }
    let ring: Vec<DVec3> = face
        .vertices
        .iter()
        .map(|v| Vec3::from(v.position).as_dvec3())
        .collect();
    let whole: f64 = (1..ring.len() - 1)
        .map(|i| signed_twice_area(ring[0], ring[i], ring[i + 1], DVec3::Y))
        .sum();
    assert!(
        (whole - tiled).abs() <= 1.0e-6 * whole,
        "face {whole}, triangles {tiled}"
    );
    if let Some(vertex) = corner.iter().position(|&is_corner| !is_corner) {
        panic!("vertex {vertex} is no triangle corner");
    }
}

/// How [`a_face_with_thousands_of_inserted_vertices_packs_within_n_log_n_install_steps`]
/// grids its face.
#[derive(Debug, Clone, Copy, PartialEq)]
enum InsertedGrid {
    /// No lattice.
    None,
    /// Planes every 8 m: only z = 8 crosses the face, so one part holds every
    /// insert.
    OnePlane,
    /// z planes spaced so about one crosses the inserted edge per four
    /// inserts: the plane count grows with the vertex count (P24).
    EveryPlane,
}

impl InsertedGrid {
    fn lattice(self, inserts: usize) -> Option<FluidGridLattice> {
        match self {
            Self::None => None,
            Self::OnePlane => FluidGridLattice::new([0.0; 3], [16.0; 3]),
            Self::EveryPlane => {
                let spacing = 6.9 / (inserts.max(4) / 4) as f32;
                FluidGridLattice::new([0.0; 3], [16.0, 16.0, 2.0 * spacing])
            }
        }
    }
}

/// Reflex vertices of an up-facing face's ring: those turning clockwise seen
/// from +Y past the ear clip's corner threshold.
fn reflex_vertex_count(face: &EnvironmentSurfaceFace) -> usize {
    let ring: Vec<DVec3> = face
        .vertices
        .iter()
        .map(|v| Vec3::from(v.position).as_dvec3())
        .collect();
    let count = ring.len();
    (0..count)
        .filter(|&i| {
            let (prev, at, next) = (
                ring[(i + count - 1) % count],
                ring[i],
                ring[(i + 1) % count],
            );
            let (into, out) = (at - prev, next - at);
            into.cross(out).dot(DVec3::Y) < -1.0e-12 * into.length() * out.length()
        })
        .count()
}

// Closure can give one face thousands of vertices on an edge, each within
// 0.1 mm of it. Such a face, its inserts on the edge or every other one a
// dent inside it, packs whole, ungridded and gridded (one plane crossing it,
// so one part holds every insert; and every plane along its inserted edge
// crossing it, so the planes grow with the vertices), keeps the drift map of
// the same face without them, and its install work, counted, grows within
// n log n of its vertex count, the grid cut and the ear clip's containment
// index included.
#[test]
fn a_face_with_thousands_of_inserted_vertices_packs_within_n_log_n_install_steps() {
    let pack = |face: &EnvironmentSurfaceFace, lattice: Option<&FluidGridLattice>| {
        INSTALL_STEPS.with(|steps| steps.set(0));
        let mesh = pack_fluid_surfaces(&[volume(vec![face.clone()])], &[[64, 64]], 1, lattice);
        (mesh, INSTALL_STEPS.with(std::cell::Cell::get))
    };
    let bare = inserted_top(0, 0.0);
    let corners: Vec<DVec3> = bare
        .vertices
        .iter()
        .map(|v| Vec3::from(v.position).as_dvec3())
        .collect();
    for (dent, grid) in [0.0, INSERT_DENT].into_iter().flat_map(|dent| {
        [
            InsertedGrid::OnePlane,
            InsertedGrid::EveryPlane,
            InsertedGrid::None,
        ]
        .map(|grid| (dent, grid))
    }) {
        let mut counted = Vec::new();
        for inserts in [2000, 8000] {
            let lattice = grid.lattice(inserts);
            let (plain, _) = pack(&bare, lattice.as_ref());
            let plain_map = plain.face_drift_maps[0].expect("the face draws");
            let face = inserted_top(inserts, dent);
            if dent > 0.0 {
                assert_eq!(
                    reflex_vertex_count(&face),
                    inserts / 2,
                    "every other insert dents"
                );
            }
            let (mesh, steps) = pack(&face, lattice.as_ref());
            assert_eq!(mesh.grid.is_some(), lattice.is_some());
            match grid {
                InsertedGrid::OnePlane => {
                    assert!(
                        mesh.vertices.iter().any(|v| v.position[2] == 8.0),
                        "the z = 8 plane cuts the face"
                    );
                    // From the first insert to the corner after the last; no
                    // plane crosses there.
                    assert_tiles_up_facing_face(&mesh, &face, 3..3 + inserts as u32);
                }
                InsertedGrid::EveryPlane => {
                    let spacing = mesh.grid.expect("gridded").spacing().z;
                    let planes = (6.9 / spacing) as usize;
                    assert!(
                        planes >= inserts / 5,
                        "{planes} planes cross the inserted edge"
                    );
                    // Planes cut the inserted edge's segments, so only the
                    // tiling's area and winding are checked.
                    assert_tiles_up_facing_face(&mesh, &face, 3..3);
                }
                InsertedGrid::None => {
                    assert_tiles_up_facing_face(&mesh, &face, 3..3 + inserts as u32);
                }
            }

            // Within the fit tolerance of the bare face's map at its corners.
            let map = mesh.face_drift_maps[0].expect("the face draws");
            assert_eq!(map.vertex_count as usize, mesh.vertices.len());
            for corner in &corners {
                let delta = *corner - corners[0];
                for axis in 0..2 {
                    let gap =
                        map.uv_gradient[axis].as_dvec3() - plain_map.uv_gradient[axis].as_dvec3();
                    let texels = gap.dot(delta).abs() * 64.0;
                    assert!(
                        texels <= drift::UV_TOLERANCE_TEXELS,
                        "{inserts} inserts, dent {dent}: map parts by {texels} texels at {corner}"
                    );
                }
            }
            counted.push((face.vertices.len(), steps));
        }
        let [(small, small_steps), (large, large_steps)] = counted[..] else {
            unreachable!("two sizes");
        };
        assert!(
            small_steps >= small as u64,
            "the count reaches the fit and the triangulation"
        );
        let n_log_n = |n: usize| n as f64 * (n as f64).log2();
        let ratio = large_steps as f64 / small_steps as f64;
        let bound = n_log_n(large) / n_log_n(small);
        assert!(
            ratio <= bound,
            "{grid:?}, dent {dent}: {small_steps} steps at {small} vertices, {large_steps} at \
             {large}: ratio {ratio}, n log n allows {bound}"
        );
    }
}

// Regression (review): the face normal was a Newell sum in f32 on absolute
// positions. On a narrow strip far from the origin with thousands of inserts
// on one long edge, its rounding outgrew twice the strip's area and flipped
// the normal down: the strip's ear clip, gridded or not, then clips nothing,
// and its area centroid falls back to the vertex mean.
#[test]
fn a_narrow_strip_far_out_with_thousands_of_inserts_packs_facing_up() {
    // 2 cm × 50 m at x ≈ 1000.37, CCW seen from +Y, whose x = x1 edge
    // carries the inserts from z 51 down to 1. Positions and texel UVs are
    // computed in f64 and rounded to f32 separately, as `inserted_top`'s are.
    let inserts: usize = 4000;
    let (x0, x1, z0, z1) = (1000.37, 1000.39, 1.0, 51.0);
    let mut points = vec![DVec2::new(x0, z0), DVec2::new(x0, z1), DVec2::new(x1, z1)];
    points.extend(
        (0..inserts).map(|i| DVec2::new(x1, z1 - (z1 - z0) * (i as f64 + 0.5) / inserts as f64)),
    );
    points.push(DVec2::new(x1, z0));
    let face = EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: points
            .iter()
            .map(|p| {
                vertex(
                    [p.x as f32, 1.1, p.y as f32],
                    (*p / 0.0254).as_vec2().to_array(),
                )
            })
            .collect(),
    };
    let normal = face_normal(&face).expect("the strip has area");
    assert!(normal.abs_diff_eq(Vec3::Y, 1.0e-6), "{normal:?}");

    // Planes every 8 m: z = 8 to 48 cross the strip, its inserted edge too.
    for lattice in [None, FluidGridLattice::new([0.0; 3], [16.0; 3])] {
        let mesh = pack_fluid_surfaces(
            &[volume(vec![face.clone()])],
            &[[64, 64]],
            1,
            lattice.as_ref(),
        );
        assert_eq!(mesh.grid.is_some(), lattice.is_some());
        for packed in &mesh.vertices {
            let packed = Vec3::from(packed.normal);
            assert!(packed.abs_diff_eq(Vec3::Y, 1.0e-6), "{packed:?}");
        }
        if lattice.is_some() {
            assert!(
                mesh.vertices.iter().any(|v| v.position[2] == 8.0),
                "the z = 8 plane cuts the strip"
            );
            // Planes cut the inserted edge's segments, so only the tiling's
            // area and winding are checked.
            assert_tiles_up_facing_face(&mesh, &face, 3..3);
        } else {
            assert_tiles_up_facing_face(&mesh, &face, 3..3 + inserts as u32);
        }
        assert!(
            mesh.face_drift_maps[0].is_some(),
            "the strip takes a drift map"
        );
    }
}

// A face with an inserted vertex packs its own vertices alone, each with its
// own normalized UV, and the drift map covers exactly them. Its area
// centroid, the drift sample, stays at the square's centre: the insert adds
// no area.
#[test]
fn packing_a_face_with_an_inserted_vertex_adds_no_vertex() {
    let mut face = top_face(0, [0.0; 3], 1.0);
    face.vertices
        .insert(1, vertex([0.0, 1.0, 0.25], [0.0, 8.0]));
    let mesh = pack_fluid_surfaces(&[volume(vec![face.clone()])], &[[64, 32]], 1, None);
    let uvs: Vec<[f32; 2]> = mesh.vertices.iter().map(|v| v.uv).collect();
    assert_eq!(
        uvs,
        [[0.0, 0.0], [0.0, 0.25], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]]
    );
    assert_eq!(mesh.indices.len(), 3 * 3);
    assert_eq!(mesh.face_drift_maps[0].expect("draws").vertex_count, 5);
    let centroid = face_area_centroid(&face, Vec3::Y);
    assert!(
        centroid.abs_diff_eq(DVec3::new(0.5, 1.0, 0.5), 1.0e-12),
        "{centroid:?}"
    );
}

// Regression (review round 2): a 1 m × 10 mm strip whose top edge holds an
// insert 0.08 mm inside it, 5 mm from a corner, is simple and within 0.1 mm
// of convex but not star-shaped about its area centroid (0.5 m, 5 mm). The
// insert lies past the ray from the centroid c through the near corner: in
// the short edge's angular sector. The centroid fan's triangle over insert →
// corner then winds backward, twice-area (insert − c) × (corner − c) =
// (−0.495, 0.00492) × (−0.5, 0.005) ≈ −1.5e-5 m²: about 7.5 mm² drawn
// inverted over its neighbour, a hairline double blend half a metre long.
// Ear clipping over the strip's own vertices draws every triangle facing up.
#[test]
fn a_thin_strip_with_an_insert_inside_its_long_edge_near_a_corner_packs_facing_up() {
    // CCW seen from +Y, in-plane (x, v) with v up; the insert at (0.005,
    // 0.00992) on the top edge from (1, 0.01) back to (0, 0.01).
    let strip = level_face(&[
        [0.0, 0.0],
        [1.0, 0.0],
        [1.0, 0.01],
        [0.005, 0.00992],
        [0.0, 0.01],
    ]);
    let normal = face_normal(&strip).expect("the strip has area");
    assert!(normal.abs_diff_eq(Vec3::Y, 1.0e-6), "{normal:?}");
    let ring: Vec<DVec3> = strip
        .vertices
        .iter()
        .map(|v| Vec3::from(v.position).as_dvec3())
        .collect();
    // The old fan's triangle inverts: the premise this guards.
    let centroid = face_area_centroid(&strip, normal);
    assert!(signed_twice_area(centroid, ring[3], ring[4], DVec3::Y) < 0.0);

    // Packed ungridded, every triangle winds up and the areas sum to the
    // strip's, over its own vertices alone: no triangle inverts. The clip
    // order follows the ring, so each vertex leads once.
    for lead in 0..strip.vertices.len() {
        let mut rotated = strip.clone();
        rotated.vertices.rotate_left(lead);
        assert_tiles_face_without_t_junctions(&rotated);
    }
}

// Regression (review round 3): the collinear tolerance shrank to 1% of the
// face's extent under 2 cm across, so on a 6 mm square an insert 0.09 mm
// inside an edge (over the 0.085 mm that left) was no collinear vertex. The
// square took the plain fan from vertex 0, and with either corner of that
// edge leading, the fan triangle over the insert wound backward: about
// 0.27 mm² drawn inverted. The tolerance is absolute now, so the square
// ear-clips facing up, its area whole, in every rotation.
#[test]
fn a_six_millimetre_square_with_an_insert_inside_an_edge_packs_facing_up() {
    let square = level_face(&[
        [0.0, 0.0],
        [0.003, 0.00009],
        [0.006, 0.0],
        [0.006, 0.006],
        [0.0, 0.006],
    ]);
    let ring: Vec<DVec3> = square
        .vertices
        .iter()
        .map(|v| Vec3::from(v.position).as_dvec3())
        .collect();
    // The plain fan from the corner before the insert inverts: the premise
    // this guards.
    assert!(signed_twice_area(ring[0], ring[1], ring[2], DVec3::Y) < 0.0);
    for lead in 0..square.vertices.len() {
        let mut rotated = square.clone();
        rotated.vertices.rotate_left(lead);
        assert_tiles_face_without_t_junctions(&rotated);
    }
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
// its volume has a probe-bound face, its reflectivity is above 0, and the
// frame's lighting mask holds every bit of `LightTermMask::ALL`; every other
// draw takes the reflection-free variant. Opacity and fog never choose it.
#[test]
fn shader_variant_reflects_only_a_bound_face_above_zero_reflectivity_under_every_term() {
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
                        let expected = if has_bound_face && reflectivity > 0.0 && every_term {
                            FluidShaderVariant::Reflecting
                        } else {
                            FluidShaderVariant::ReflectionFree
                        };
                        assert_eq!(
                            row.shader_variant(mask),
                            expected,
                            "bound {has_bound_face}, reflectivity {reflectivity}, opacity \
                             {opacity}, fog {fog_distance:?}, mask {mask:#x}"
                        );
                    }
                }
            }
        }
    }
    assert!(FluidShaderVariant::Reflecting.reflections());
    assert!(!FluidShaderVariant::ReflectionFree.reflections());
}

// The frame's draws carry their variants in the collector's far-to-near
// order, so the pass switches pipeline only where neighbours differ; an
// isolating mask turns every draw reflection-free.
#[test]
fn slots_pair_each_frame_draw_with_its_shader_variant_in_draw_order() {
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
    let variants: Vec<_> = slots.frame_draws(all).collect();
    assert_eq!(
        variants,
        [
            (2, FluidShaderVariant::ReflectionFree),
            (0, FluidShaderVariant::Reflecting),
            (1, FluidShaderVariant::ReflectionFree),
        ]
    );
    assert!(
        slots
            .frame_draws(0x13F)
            .all(|(_, variant)| variant == FluidShaderVariant::ReflectionFree)
    );
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
