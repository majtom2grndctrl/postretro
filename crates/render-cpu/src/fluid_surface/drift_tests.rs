// Drift scroll from world drift: direction and speed under any projection,
// in-plane only, and one map per split brush side.
// See: context/lib/rendering_pipeline.md §7.3a

use glam::{DMat2, DVec2, Mat2, Vec2, Vec3};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};

use super::super::motion::drift_texels;
use super::super::pack_fluid_surfaces;
use super::*;

const DIMS: [u32; 2] = [64, 64];
/// Texels per meter at face scale 1 (1 map unit = 0.0254 m).
const TEXELS_PER_METER: f32 = 1.0 / 0.0254;

/// A texel projection: texel UV = `basis · (x, z)` for a level face.
fn projection(scale: f32, degrees: f32) -> Mat2 {
    Mat2::from_angle(degrees.to_radians()) * (TEXELS_PER_METER / scale)
}

/// An up-facing level polygon over `corners` (x, z), CCW seen from above,
/// UVs from `basis`.
fn level_face(corners: &[[f32; 2]], basis: Mat2) -> EnvironmentSurfaceFace {
    EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: corners
            .iter()
            .map(|&[x, z]| EnvironmentSurfaceVertex {
                position: [x, 0.0, z],
                uv: (basis * Vec2::new(x, z)).to_array(),
            })
            .collect(),
    }
}

fn rect<T: Copy>(x0: T, x1: T, z0: T, z1: T) -> Vec<[T; 2]> {
    vec![[x0, z1], [x1, z1], [x1, z0], [x0, z0]]
}

fn record(faces: Vec<EnvironmentSurfaceFace>) -> EnvironmentVolumeRecord {
    EnvironmentVolumeRecord {
        source_entity: 0,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".into(),
        brushes: Vec::new(),
        surface_faces: faces,
    }
}

fn maps(faces: Vec<EnvironmentSurfaceFace>) -> Vec<FluidFaceDriftMap> {
    pack_fluid_surfaces(&[record(faces)], &[DIMS], 1)
        .face_drift_maps
        .into_iter()
        .map(|map| map.expect("every fixture face draws"))
        .collect()
}

/// World velocity (x, z) at which texture features move under `rate`: a
/// feature shows where `uv + scroll = u0`, so its UV moves at `-rate`.
fn feature_velocity(rate: Vec2, basis: Mat2) -> Vec2 {
    basis.inverse() * (-rate * Vec2::from(DIMS.map(|d| d as f32)))
}

#[test]
fn drift_moves_texture_features_with_the_push_at_its_true_speed() {
    let dims = Vec2::from(DIMS.map(|d| d as f32));
    for (scale, degrees) in [(1.0, 0.0), (0.25, 0.0), (1.0, 45.0), (0.25, 45.0)] {
        let basis = projection(scale, degrees);
        let map = maps(vec![level_face(&rect(0.0, 2.0, 0.0, 2.0), basis)])[0];
        for drift in [Vec3::new(3.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 2.0)] {
            let rate = map.scroll_rate(drift);
            let feature = feature_velocity(rate, basis);
            let want = Vec2::new(drift.x, drift.z);
            assert!(
                (feature - want).length() < 1.0e-3,
                "scale {scale} at {degrees}°: features move {feature}, push carries {want}"
            );
            // Reversing the push reverses the scroll.
            assert!((map.scroll_rate(-drift) + rate).length() < 1.0e-6);

            // The whole-texel scroll covers the true distance over 10 s.
            let travelled = drift_texels(rate, 10.0, dims) - drift_texels(rate, 0.0, dims);
            let expected = rate * dims * 10.0;
            let wrapped = travelled - ((travelled - expected) / dims).round() * dims;
            assert!(
                (wrapped - expected).abs().max_element() <= 1.0,
                "{wrapped} vs {expected}"
            );
        }
    }
}

#[test]
fn only_the_in_plane_drift_scrolls_a_face() {
    let basis = projection(1.0, 0.0);
    let level = maps(vec![level_face(&rect(0.0, 1.0, 0.0, 1.0), basis)])[0];
    assert_eq!(level.scroll_rate(Vec3::new(0.0, 3.0, 0.0)), Vec2::ZERO);
    assert!(
        (level.scroll_rate(Vec3::new(2.0, 5.0, 1.0)) - level.scroll_rate(Vec3::new(2.0, 0.0, 1.0)))
            .length()
            < 1.0e-6
    );

    // A wall facing +X, UVs from (z, y): the X part of a drift never scrolls it.
    let wall = EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]]
            .iter()
            .map(|&[z, y]: &[f32; 2]| EnvironmentSurfaceVertex {
                position: [0.0, y, -z],
                uv: [z * TEXELS_PER_METER, -y * TEXELS_PER_METER],
            })
            .collect(),
    };
    let wall = maps(vec![wall])[0];
    let along = Vec3::new(0.0, 2.0, -1.0);
    assert!(
        (wall.scroll_rate(along + Vec3::new(7.0, 0.0, 0.0)) - wall.scroll_rate(along)).length()
            < 1.0e-6
    );
    assert_ne!(wall.scroll_rate(along), Vec2::ZERO);
}

/// An up-facing level polygon over `corners` (x, z) at `height`, as the
/// compiler emits one: each vertex's position and texel UV computed in f64
/// from one rotated projection, then rounded to f32 separately.
fn compiled_level_face(
    corners: &[[f64; 2]],
    height: f64,
    scale: f64,
    degrees: f64,
) -> EnvironmentSurfaceFace {
    let basis = DMat2::from_angle(degrees.to_radians()) * (f64::from(TEXELS_PER_METER) / scale);
    EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: corners
            .iter()
            .map(|&[x, z]| EnvironmentSurfaceVertex {
                position: [x as f32, height as f32, z as f32],
                uv: (basis * DVec2::new(x, z)).as_vec2().to_array(),
            })
            .collect(),
    }
}

#[test]
fn every_part_of_a_split_face_drifts_identically_however_thin() {
    let drift = Vec3::new(3.0, 0.0, -1.5);
    // Hundreds of meters out at fine scales, texel UVs reach ~10^5, where f32
    // steps by up to 1/64 texel: thin parts miss their side's map by more
    // than 0.01 texel.
    for (scale, x0, z0) in [(0.25, 300.37, -420.81), (0.125, 480.19, 350.63)] {
        let height = 12.7;
        let piece = |from: f64, to: f64| {
            compiled_level_face(&rect(x0 + from, x0 + to, z0, z0 + 1.0), height, scale, 45.0)
        };
        // One side split into a 1 m part, a 1 mm part, a 0.2 mm part and the rest.
        let parts = maps(vec![
            piece(0.0, 1.0),
            piece(1.0, 1.001),
            piece(1.001, 1.0012),
            piece(1.0012, 2.0),
        ]);
        let rate = parts[0].scroll_rate(drift);
        for (index, part) in parts.iter().enumerate().skip(1) {
            assert_eq!(
                part.uv_gradient, parts[0].uv_gradient,
                "scale {scale} at ({x0}, {z0}): part {index} took its own map"
            );
            assert_eq!(part.scroll_rate(drift), rate);
        }

        // A face with another projection keeps its own map, even on the same plane.
        let separate = maps(vec![
            piece(0.0, 1.0),
            compiled_level_face(&rect(x0 + 3.0, x0 + 4.0, z0, z0 + 1.0), height, scale, 0.0),
        ]);
        assert_ne!(separate[0].uv_gradient, separate[1].uv_gradient);
    }

    // Near the origin too.
    let basis = projection(0.25, 45.0);
    let shifted = level_face(&rect(3.0, 4.0, 0.0, 1.0), projection(1.0, 0.0));
    let separate = maps(vec![level_face(&rect(0.0, 1.0, 0.0, 1.0), basis), shifted]);
    assert_ne!(separate[0].uv_gradient, separate[1].uv_gradient);
}

#[test]
fn drift_rates_fill_every_vertex_a_face_owns_and_a_mismatched_slice_leaves_all_still() {
    let basis = projection(1.0, 0.0);
    // The second face's collinear vertex gains it a centroid vertex, which
    // must scroll with the face: the shader reads the rate flat.
    let mut with_insert = rect(1.0, 2.0, 0.0, 1.0);
    with_insert.insert(1, [1.5, 1.0]);
    let mesh = pack_fluid_surfaces(
        &[record(vec![
            level_face(&rect(0.0, 1.0, 0.0, 1.0), basis),
            level_face(&with_insert, basis),
        ])],
        &[DIMS],
        1,
    );
    assert_eq!(mesh.vertices.len(), 4 + 5 + 1);
    assert_eq!(mesh.face_drift_maps[0].unwrap().vertex_count, 4);
    let second = mesh.face_drift_maps[1].unwrap();
    assert_eq!(second.vertex_count, 5 + 1);
    let start = second.vertex_start as usize;
    let centroid = mesh.vertices[start + 5].position;
    assert!(with_insert.iter().all(|&[x, z]| [x, 0.0, z] != centroid));

    let mut rates = vec![[9.0; 2]; mesh.vertices.len()];
    fill_drift_rates(
        &mesh.face_drift_maps,
        &[Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0)],
        &mut rates,
    );
    let rate = second.scroll_rate(Vec3::new(3.0, 0.0, 0.0)).to_array();
    assert_ne!(rate, [0.0; 2]);
    for (index, vertex_rate) in rates.iter().enumerate() {
        if (start..start + 6).contains(&index) {
            assert_eq!(*vertex_rate, rate, "vertex {index}");
        } else {
            assert_eq!(*vertex_rate, [0.0; 2], "vertex {index}");
        }
    }
    fill_drift_rates(&mesh.face_drift_maps, &[Vec3::X], &mut rates);
    assert!(rates.iter().all(|rate| *rate == [0.0; 2]));
}
