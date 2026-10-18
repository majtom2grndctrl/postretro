// Fluid surface grid packing tests: cuts, shared edges and the vertex cap.
// See: context/lib/testing_guide.md

use std::ops::Range;

use glam::{DVec2, DVec3, Vec2, Vec3};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};
use postretro_test_log_capture::LogCapture;

use super::super::{
    FluidSurfaceMesh, fill_drift_rates, pack_fluid_surfaces, pack_fluid_surfaces_within,
};
use super::*;

const ORIGIN: [f32; 3] = [0.25, -0.5, 0.125];
/// Half the 1 m cell: the finest spacing.
const SPACING: f64 = 0.5;

fn lattice() -> FluidGridLattice {
    FluidGridLattice::new(ORIGIN, [1.0; 3]).expect("a 1 m lattice is usable")
}

/// Texel UVs affine in position, distinct per axis, so a wrong interpolation
/// shows on any edge.
fn affine_uv(p: [f32; 3]) -> [f32; 2] {
    [
        64.0 * p[0] + 8.0 * p[2] + 3.0,
        16.0 * p[1] - 32.0 * p[2] + 5.0,
    ]
}

fn face(points: &[[f32; 3]]) -> EnvironmentSurfaceFace {
    EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: points
            .iter()
            .map(|&position| EnvironmentSurfaceVertex {
                position,
                uv: affine_uv(position),
            })
            .collect(),
    }
}

/// A pool top at height `y` over `x0..x1` × `z0..z1`, CCW seen from +Y.
fn top(x0: f32, x1: f32, z0: f32, z1: f32, y: f32) -> EnvironmentSurfaceFace {
    face(&[[x0, y, z0], [x0, y, z1], [x1, y, z1], [x1, y, z0]])
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

fn pack(volumes: &[EnvironmentVolumeRecord]) -> FluidSurfaceMesh {
    pack_fluid_surfaces(volumes, &[], 1, Some(&lattice()))
}

fn dvec(p: [f32; 3]) -> DVec3 {
    Vec3::from(p).as_dvec3()
}

/// Mesh vertex range of the `index`th surface face over every volume.
fn face_range(mesh: &FluidSurfaceMesh, index: usize) -> Range<usize> {
    let map = mesh.face_drift_maps[index].expect("fixture faces draw");
    map.vertex_start as usize..(map.vertex_start + map.vertex_count) as usize
}

fn face_triangles(mesh: &FluidSurfaceMesh, range: &Range<usize>) -> Vec<[u32; 3]> {
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .copied()
        .filter(|t| t.iter().all(|&i| range.contains(&(i as usize))))
        .collect()
}

fn on_some_plane(p: DVec3, spacing: f64) -> bool {
    (0..3).any(|axis| {
        let origin = f64::from(ORIGIN[axis]);
        let k = ((p[axis] - origin) / spacing).round();
        (p[axis] - (origin + k * spacing)).abs() <= 1.0e-5 * (1.0 + p[axis].abs())
    })
}

/// Asserts what every gridded face holds, and returns its mesh range: the
/// face's own vertices lead unchanged; every vertex lies in the face's plane
/// and polygon, has the face's affine UV and is unique; every added vertex
/// lies on a lattice plane at `spacing`; every triangle winds with the face,
/// has area and spans at most one spacing (plus the plane tolerance) on each
/// axis; the triangles' areas sum to the face's; every vertex is a corner;
/// and no triangle edge passes through another of the face's vertices.
fn assert_gridded_face(
    mesh: &FluidSurfaceMesh,
    index: usize,
    face: &EnvironmentSurfaceFace,
    spacing: f64,
) -> Range<usize> {
    let range = face_range(mesh, index);
    let ring = face.vertices.len();
    let position = |i: usize| dvec(mesh.vertices[i].position);
    for (offset, source) in face.vertices.iter().enumerate() {
        assert_eq!(mesh.vertices[range.start + offset].position, source.position);
    }
    let normal = face_normal(face).unwrap().as_dvec3();
    let corners: Vec<DVec3> = face.vertices.iter().map(|v| dvec(v.position)).collect();
    let (a, b, c) = (corners[0], corners[1], corners[2]);
    let uv_at = |p: DVec3| {
        // The fixture UVs are affine in position, so the face's map is the
        // fixture's own.
        Vec2::from(affine_uv(p.as_vec3().to_array())).as_dvec2()
    };
    for i in range.clone() {
        let p = position(i);
        assert!((p - a).dot(normal).abs() <= 1.0e-5, "vertex {i} off the plane");
        for edge in 0..ring {
            let (e0, e1) = (corners[edge], corners[(edge + 1) % ring]);
            assert!(
                (e1 - e0).cross(p - e0).dot(normal) >= -1.0e-5,
                "vertex {i} outside the face"
            );
        }
        let uv = Vec2::from(mesh.vertices[i].uv).as_dvec2();
        assert!(
            (uv - uv_at(p)).abs().max_element() <= 1.0e-3,
            "vertex {i} uv {uv} vs {}",
            uv_at(p)
        );
        if i >= range.start + ring {
            assert!(on_some_plane(p, spacing), "added vertex {p} on no plane");
        }
        for j in range.start..i {
            assert_ne!(
                mesh.vertices[j].position, mesh.vertices[i].position,
                "vertices {j} and {i} repeat a position"
            );
        }
    }
    let triangles = face_triangles(mesh, &range);
    let mut tiled = 0.0;
    for t in &triangles {
        let [p, q, r] = t.map(|i| position(i as usize));
        let twice = (q - p).cross(r - p).dot(normal);
        assert!(twice > 0.0, "triangle {t:?} has twice-area {twice}");
        tiled += twice;
        let span = p.max(q).max(r) - p.min(q).min(r);
        assert!(
            span.max_element() <= spacing + 2.0 * PLANE_TOLERANCE + 1.0e-6,
            "triangle {t:?} spans {span}"
        );
        for side in 0..3 {
            let (s0, s1) = (position(t[side] as usize), position(t[(side + 1) % 3] as usize));
            for w in range.clone().filter(|w| !t.contains(&(*w as u32))) {
                let w = position(w);
                let along = (w - s0).dot(s1 - s0) / (s1 - s0).length_squared();
                let off = (w - s0).cross(s1 - s0).length() / (s1 - s0).length();
                assert!(
                    off > 1.0e-6 || along <= 1.0e-9 || along >= 1.0 - 1.0e-9,
                    "triangle edge of {t:?} spans {w}"
                );
            }
        }
    }
    let face_twice: f64 = (1..ring - 1)
        .map(|i| (corners[i] - a).cross(corners[i + 1] - a).dot(normal))
        .sum();
    assert!(
        (face_twice - tiled).abs() <= 1.0e-6 * face_twice,
        "face {face_twice}, triangles {tiled}"
    );
    let _ = (b, c);
    for i in range.clone() {
        assert!(
            triangles.iter().any(|t| t.contains(&(i as u32))),
            "vertex {i} is no triangle corner"
        );
    }
    range
}

/// The face's vertices on segment `a`–`b`, ordered from `a`.
fn edge_vertices(mesh: &FluidSurfaceMesh, range: &Range<usize>, a: [f32; 3], b: [f32; 3]) -> Vec<[f32; 3]> {
    let (a, b) = (dvec(a), dvec(b));
    let mut on: Vec<(f64, [f32; 3])> = range
        .clone()
        .map(|i| mesh.vertices[i].position)
        .filter_map(|p| {
            let q = dvec(p);
            let along = (q - a).dot(b - a) / (b - a).length_squared();
            let off = (q - a).cross(b - a).length() / (b - a).length();
            (off <= 1.0e-6 && (-1.0e-9..=1.0 + 1.0e-9).contains(&along)).then_some((along, p))
        })
        .collect();
    on.sort_by(|x, y| x.0.total_cmp(&y.0));
    on.into_iter().map(|(_, p)| p).collect()
}

#[test]
fn a_face_spanning_several_cells_is_cut_into_parts_no_wider_than_the_spacing() {
    let pool = top(0.0, 2.3, -0.4, 1.9, 1.1);
    let mesh = pack(&[volume(vec![pool.clone()])]);
    assert_eq!(mesh.grid.unwrap().spacing(), DVec3::splat(SPACING));
    let range = assert_gridded_face(&mesh, 0, &pool, SPACING);
    // x planes 0.25 … 2.25 (5), z planes −0.375 … 1.625 (5): a rectangle
    // reaches its bound exactly.
    assert_eq!(range.len(), 4 + 2 * (5 + 5) + 5 * 5);
}

#[test]
fn a_sloped_face_is_cut_by_every_axis_crossing_it() {
    // A parallelogram descending in y while running in x and z.
    let ramp = face(&[
        [2.9, 1.1, 0.0],
        [2.9, 1.1, 2.0],
        [4.2, -0.3, 2.6],
        [4.2, -0.3, 0.6],
    ]);
    let mesh = pack(&[volume(vec![ramp.clone()])]);
    let range = assert_gridded_face(&mesh, 0, &ramp, SPACING);
    let added = &mesh.vertices[range.start + 4..range.end];
    for axis in 0..3 {
        let origin = f64::from(ORIGIN[axis]);
        assert!(
            added.iter().any(|v| {
                let p = f64::from(v.position[axis]);
                ((p - origin) / SPACING - ((p - origin) / SPACING).round()).abs() < 1.0e-5
            }),
            "no vertex on an axis-{axis} plane"
        );
    }
}

#[test]
fn a_face_no_plane_crosses_packs_as_without_a_grid() {
    // Inside one half-metre cell on every axis.
    let small = top(0.3, 0.7, 0.2, 0.6, 1.1);
    let mut inserted = small.clone();
    inserted.vertices.insert(
        1,
        EnvironmentSurfaceVertex {
            position: [0.3, 1.1, 0.4],
            uv: affine_uv([0.3, 1.1, 0.4]),
        },
    );
    for faces in [vec![small], vec![inserted]] {
        let volumes = [volume(faces)];
        let gridded = pack(&volumes);
        let plain = pack_fluid_surfaces(&volumes, &[], 1, None);
        assert!(gridded.grid.is_some());
        assert_eq!(gridded.vertices, plain.vertices);
        assert_eq!(gridded.indices, plain.indices);
    }
}

// P9: an edge shared by faces of two volumes, by a top and a side, and by a
// top and a sloped face, in either winding, carries identical vertices.
#[test]
fn faces_sharing_an_edge_carry_identical_vertices_along_it() {
    let left = top(0.0, 1.3, 0.0, 2.0, 1.1);
    let right = top(1.3, 2.9, 0.0, 2.0, 1.1);
    // The side wall under the left top's z = 0 edge, facing −z.
    let side = face(&[[0.0, 1.1, 0.0], [1.3, 1.1, 0.0], [1.3, -0.6, 0.0], [0.0, -0.6, 0.0]]);
    let ramp = face(&[
        [2.9, 1.1, 0.0],
        [2.9, 1.1, 2.0],
        [4.2, -0.3, 2.6],
        [4.2, -0.3, 0.6],
    ]);
    // A face below the ramp's oblique edge, which crosses planes of all
    // three axes, wound the other way along it.
    let under = face(&[[2.9, 1.1, 0.0], [4.2, -0.3, 0.6], [3.5, -1.0, -1.0]]);
    let volumes = [
        volume(vec![left.clone(), side.clone()]),
        volume(vec![right.clone(), ramp.clone(), under.clone()]),
    ];
    let mesh = pack(&volumes);
    let ranges: Vec<_> = [&left, &side, &right, &ramp, &under]
        .iter()
        .enumerate()
        .map(|(i, f)| assert_gridded_face(&mesh, i, f, SPACING))
        .collect();
    let shared = |a: usize, b: usize, p: [f32; 3], q: [f32; 3]| {
        let from_a = edge_vertices(&mesh, &ranges[a], p, q);
        let from_b = edge_vertices(&mesh, &ranges[b], p, q);
        assert!(from_a.len() > 2, "the planes cross edge {p:?}–{q:?}");
        assert_eq!(from_a, from_b, "edge {p:?}–{q:?}");
    };
    shared(0, 2, [1.3, 1.1, 0.0], [1.3, 1.1, 2.0]);
    shared(0, 1, [0.0, 1.1, 0.0], [1.3, 1.1, 0.0]);
    shared(2, 3, [2.9, 1.1, 0.0], [2.9, 1.1, 2.0]);
    shared(3, 4, [2.9, 1.1, 0.0], [4.2, -0.3, 0.6]);
}

// P11: an edge lying in a lattice plane gains nothing from that plane, and
// no sliver strip runs beside it.
#[test]
fn an_edge_in_a_lattice_plane_gains_no_vertex_from_it() {
    let on_plane = top(1.25, 2.6, 0.0, 1.0, 1.1);
    let neighbour = top(0.4, 1.25, 0.0, 1.0, 1.1);
    let mesh = pack(&[volume(vec![on_plane.clone(), neighbour.clone()])]);
    let ranges = [
        assert_gridded_face(&mesh, 0, &on_plane, SPACING),
        assert_gridded_face(&mesh, 1, &neighbour, SPACING),
    ];
    let edge = |range| edge_vertices(&mesh, range, [1.25, 1.1, 0.0], [1.25, 1.1, 1.0]);
    // Its own two corners and the z planes at 0.125 and 0.625 only.
    let expected = vec![
        [1.25, 1.1, 0.0],
        [1.25, 1.1, 0.125],
        [1.25, 1.1, 0.625],
        [1.25, 1.1, 1.0],
    ];
    assert_eq!(edge(&ranges[0]), expected);
    assert_eq!(edge(&ranges[1]), expected);
    for range in &ranges {
        for t in face_triangles(&mesh, range) {
            let xs = t.map(|i| mesh.vertices[i as usize].position[0]);
            let width = xs.iter().copied().fold(f32::MIN, f32::max)
                - xs.iter().copied().fold(f32::MAX, f32::min);
            assert!(width > 0.01, "sliver triangle {t:?} beside the plane");
        }
    }
}

// P10 and P12: a vertex within the plane tolerance of a plane takes the
// plane through itself on both faces (no crossing beside it, no duplicate);
// one just outside keeps the crossing, on both faces.
#[test]
fn crossings_near_a_shared_vertex_snap_the_same_way_from_both_faces() {
    let inside = 0.625 + 1.0e-4; // within the 2e-4 m tolerance of z = 0.625
    let outside = 1.625 + 3.0e-4; // just past it
    let exact = 1.125; // exactly on a plane
    let left = face(&[
        [0.0, 1.1, inside],
        [0.0, 1.1, outside],
        [1.3, 1.1, outside],
        [1.3, 1.1, exact],
        [1.3, 1.1, inside],
    ]);
    let right = face(&[
        [1.3, 1.1, inside],
        [1.3, 1.1, exact],
        [1.3, 1.1, outside],
        [2.6, 1.1, outside],
        [2.6, 1.1, inside],
    ]);
    let mesh = pack(&[volume(vec![left.clone()]), volume(vec![right.clone()])]);
    let ranges = [
        assert_gridded_face(&mesh, 0, &left, SPACING),
        assert_gridded_face(&mesh, 1, &right, SPACING),
    ];
    let edge = |range| edge_vertices(&mesh, range, [1.3, 1.1, inside], [1.3, 1.1, outside]);
    let along: Vec<f32> = edge(&ranges[0]).iter().map(|p| p[2]).collect();
    assert_eq!(along, vec![inside, exact, 1.625, outside]);
    assert_eq!(edge(&ranges[0]), edge(&ranges[1]));
}

// P13: an edge the compiler left with a T-junction keeps it, gains no other
// vertex there, and its two sides' crossings agree within the tolerance.
#[test]
fn an_open_t_junction_keeps_its_vertex_and_the_crossings_agree_within_tolerance() {
    let long = top(0.0, 1.3, 0.0, 2.0, 1.1);
    // The neighbour's edge bends 50 µm off the long edge's line at z = 0.9,
    // outward, so the neighbour stays convex.
    let t_vertex = [1.3 - 5.0e-5, 1.1, 0.9];
    let split = face(&[
        [1.3, 1.1, 0.0],
        t_vertex,
        [1.3, 1.1, 2.0],
        [2.9, 1.1, 2.0],
        [2.9, 1.1, 0.0],
    ]);
    let mesh = pack(&[volume(vec![long.clone()]), volume(vec![split.clone()])]);
    let ranges = [
        assert_gridded_face(&mesh, 0, &long, SPACING),
        assert_gridded_face(&mesh, 1, &split, SPACING),
    ];
    let near_edge = |range: &Range<usize>| -> Vec<[f32; 3]> {
        let mut on: Vec<[f32; 3]> = range
            .clone()
            .map(|i| mesh.vertices[i].position)
            .filter(|p| (p[0] - 1.3).abs() <= 1.0e-4)
            .collect();
        on.sort_by(|a, b| a[2].total_cmp(&b[2]));
        on
    };
    let (from_long, from_split) = (near_edge(&ranges[0]), near_edge(&ranges[1]));
    let z: Vec<f32> = from_long.iter().map(|p| p[2]).collect();
    // The long side: its corners and the z planes, nothing at the T-vertex.
    assert_eq!(z, vec![0.0, 0.125, 0.625, 1.125, 1.625, 2.0]);
    assert!(from_split.contains(&t_vertex));
    let without_t: Vec<[f32; 3]> = from_split.into_iter().filter(|&p| p != t_vertex).collect();
    assert_eq!(without_t.len(), from_long.len());
    for (a, b) in from_long.iter().zip(&without_t) {
        assert!(
            (dvec(*a) - dvec(*b)).length() <= PLANE_TOLERANCE,
            "{a:?} vs {b:?}"
        );
    }
}

// P15: each face's grid vertices stay contiguous, and its drift range covers
// them all, also after a re-resolve.
#[test]
fn each_faces_grid_vertices_stay_contiguous_under_its_drift() {
    let faces = vec![top(0.0, 1.3, 0.0, 2.0, 1.1), top(1.3, 2.9, 0.0, 2.0, 1.1)];
    let volumes = [volume(faces), volume(vec![top(5.0, 6.2, 0.0, 1.4, 1.1)])];
    let mesh = pack(&volumes);
    let ranges: Vec<_> = (0..3).map(|i| face_range(&mesh, i)).collect();
    assert_eq!(ranges[0].start, 0);
    assert_eq!(ranges[0].end, ranges[1].start);
    assert_eq!(ranges[1].end, ranges[2].start);
    assert_eq!(ranges[2].end, mesh.vertices.len());
    for (i, range) in ranges.iter().enumerate() {
        assert!(range.len() > 4, "face {i} gained grid vertices");
        for &index in &mesh.indices {
            let inside = range.contains(&(index as usize));
            assert!(
                inside || face_triangles(&mesh, range).iter().all(|t| !t.contains(&index)),
            );
        }
    }
    let mut rates = vec![[9.0; 2]; mesh.vertices.len()];
    for drift in [Vec3::X, Vec3::Z] {
        fill_drift_rates(&mesh.face_drift_maps, &[drift; 3], &mut rates);
        for range in &ranges {
            let first = rates[range.start];
            assert_ne!(first, [0.0; 2]);
            assert!(rates[range.clone()].iter().all(|&rate| rate == first));
        }
    }
}

fn record_count(capture: &LogCapture) -> usize {
    capture
        .records()
        .iter()
        .filter(|r| r.level == log::Level::Warn)
        .count()
}

/// A 4 m square top at `x0`: 8 x and 8 z planes at the finest spacing, a
/// bound of 100 vertices; 36 at 1 m, 16 at 2 m, 9 at 4 m and coarser.
fn square(x0: f32) -> EnvironmentSurfaceFace {
    top(x0, x0 + 4.0, 0.0, 4.0, 1.1)
}

// P5: a level at or under the cap keeps the finest spacing and warns not.
#[test]
fn a_level_within_the_cap_keeps_the_finest_spacing_without_a_warning() {
    let capture = LogCapture::start();
    let volumes = [volume(vec![square(0.0)]), volume(vec![square(4.0)])];
    let mesh = pack_fluid_surfaces_within(&volumes, &[], 1, Some(&lattice()), 200);
    assert_eq!(mesh.grid.unwrap().coarsening, 0);
    assert_eq!(mesh.vertices.len(), 200);
    assert_eq!(record_count(&capture), 0);
}

// P6 and P17: past the cap, every face takes the finest spacing that fits,
// chosen before any face is cut, whatever the face order; shared edges stay
// identical; the level ends within the cap; one warning however many
// spacings the search tried.
#[test]
fn a_level_past_the_cap_coarsens_every_face_alike_and_warns_once() {
    for (cap, coarsening) in [(150, 1), (20, 3)] {
        let capture = LogCapture::start();
        // The overflow arrives only with the last face, which shares the
        // first's x = 4 edge.
        let forward = [volume(vec![square(0.0)]), volume(vec![square(4.0)])];
        let reversed = [volume(vec![square(4.0)]), volume(vec![square(0.0)])];
        let mesh = pack_fluid_surfaces_within(&forward, &[], 1, Some(&lattice()), cap);
        let other = pack_fluid_surfaces_within(&reversed, &[], 1, Some(&lattice()), cap);
        let planes = mesh.grid.expect("a coarser spacing fits");
        assert_eq!(planes.coarsening, coarsening);
        assert_eq!(other.grid, mesh.grid);
        assert!(mesh.vertices.len() as u64 <= cap);
        let spacing = planes.spacing().x;
        let ranges = [
            assert_gridded_face(&mesh, 0, &square(0.0), spacing),
            assert_gridded_face(&mesh, 1, &square(4.0), spacing),
        ];
        assert_eq!(
            edge_vertices(&mesh, &ranges[0], [4.0, 1.1, 0.0], [4.0, 1.1, 4.0]),
            edge_vertices(&mesh, &ranges[1], [4.0, 1.1, 0.0], [4.0, 1.1, 4.0]),
        );
        assert_eq!(record_count(&capture), 2, "one warning per pack");
        capture.assert_logged(log::Level::Warn, "Fluid surface grid coarsened");
    }
}

// P7: ungridded vertices alone past the cap pack as without a grid, warning
// once; no spacing fitting packs ungridded too.
#[test]
fn a_level_no_spacing_fits_packs_without_a_grid_and_warns_once() {
    for cap in [3, 8] {
        let capture = LogCapture::start();
        let volumes = [volume(vec![square(0.0)])];
        let mesh = pack_fluid_surfaces_within(&volumes, &[], 1, Some(&lattice()), cap);
        let plain = pack_fluid_surfaces(&volumes, &[], 1, None);
        assert_eq!(mesh.grid, None);
        assert_eq!(mesh.vertices, plain.vertices);
        assert_eq!(mesh.indices, plain.indices);
        assert_eq!(record_count(&capture), 1);
    }
}

// P18: a face whose finest grid is far past the cap packs within it, the
// spacing chosen from its extent before the face is cut.
#[test]
fn an_ocean_face_packs_within_the_cap_at_a_spacing_from_its_extent() {
    let ocean = top(0.0, 10_000.0, 0.0, 10_000.0, 0.0);
    let capture = LogCapture::start();
    let mesh = pack(&[volume(vec![ocean])]);
    let planes = mesh.grid.expect("a coarse spacing fits");
    // 10 km / 32 m = 312 lines per axis: about 98k vertices; 16 m would need
    // about 390k.
    assert_eq!(planes.coarsening, 6);
    assert!(mesh.vertices.len() as u64 <= FLUID_GRID_VERTEX_CAP);
    assert_eq!(record_count(&capture), 1);
}

// P19: a second level chooses its spacing and warning from its own faces.
#[test]
fn a_second_level_takes_its_own_spacing_and_warning() {
    let capture = LogCapture::start();
    let first = [volume(vec![square(0.0)]), volume(vec![square(4.0)])];
    let coarse = pack_fluid_surfaces_within(&first, &[], 1, Some(&lattice()), 150);
    assert_eq!(coarse.grid.unwrap().coarsening, 1);
    assert_eq!(record_count(&capture), 1);
    capture.clear();
    let second = [volume(vec![square(0.0)])];
    let fine = pack_fluid_surfaces_within(&second, &[], 1, Some(&lattice()), 150);
    assert_eq!(fine.grid.unwrap().coarsening, 0);
    assert_eq!(record_count(&capture), 0);
}

// P14 and P4: no fluid builds no grid and logs nothing; no lattice packs as
// without a grid, silently.
#[test]
fn no_fluid_or_no_lattice_builds_no_grid_and_logs_nothing() {
    let capture = LogCapture::start();
    assert_eq!(pack(&[]).grid, None);
    let dry = EnvironmentVolumeRecord {
        fluid: String::new(),
        ..volume(Vec::new())
    };
    assert_eq!(pack(&[dry]).grid, None);
    let volumes = [volume(vec![square(0.0)])];
    let without = pack_fluid_surfaces(&volumes, &[], 1, None);
    assert_eq!(without.grid, None);
    assert_eq!(without.vertices.len(), 4);
    assert!(capture.records().is_empty(), "{:?}", capture.records());
}

// P20: a lattice with a zero, negative or non-finite cell, or a non-finite
// origin, is no lattice.
#[test]
fn an_unusable_lattice_is_refused() {
    for cell in [
        [0.0, 1.0, 1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, f32::NAN],
        [f32::INFINITY, 1.0, 1.0],
    ] {
        assert_eq!(FluidGridLattice::new([0.0; 3], cell), None, "{cell:?}");
    }
    assert_eq!(FluidGridLattice::new([f32::NAN, 0.0, 0.0], [1.0; 3]), None);
    assert!(FluidGridLattice::new([0.0; 3], [1.0e-30; 3]).is_some());
    // A vanishing cell still terminates: no spacing fits, so it packs plain.
    let tiny = FluidGridLattice::new([0.0; 3], [1.0e-30; 3]).unwrap();
    let volumes = [volume(vec![square(0.0)])];
    assert_eq!(pack_fluid_surfaces(&volumes, &[], 1, Some(&tiny)).grid, None);
}

#[test]
fn uvs_interpolate_affinely_on_crossings() {
    let pool = top(0.0, 1.3, 0.0, 2.0, 1.1);
    let mesh = pack_fluid_surfaces(&[volume(vec![pool])], &[[64, 32]], 1, Some(&lattice()));
    for v in &mesh.vertices {
        let expected = Vec2::from(affine_uv(v.position)) / Vec2::new(64.0, 32.0);
        let error = (Vec2::from(v.uv).as_dvec2() - expected.as_dvec2()).abs();
        assert!(error.max_element() <= 1.0e-5, "{v:?}");
    }
    let _ = DVec2::ZERO;
}
