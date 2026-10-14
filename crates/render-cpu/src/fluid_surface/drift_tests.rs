// Drift scroll from world drift: direction and speed under any projection,
// in-plane only, and one map per split brush side.
// See: context/lib/rendering_pipeline.md §7.3a

use glam::{Mat2, Vec2, Vec3};
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

fn rect(x0: f32, x1: f32, z0: f32, z1: f32) -> Vec<[f32; 2]> {
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

// R6: every part of one split side scrolls with one map, however thin.
#[test]
fn every_part_of_a_split_face_drifts_identically_however_thin() {
    let basis = projection(0.25, 45.0);
    let parts = maps(vec![
        level_face(&rect(0.0, 1.0, 0.0, 1.0), basis),
        // A 1 mm part beside it, offset far from the origin so f32 rounding bites.
        level_face(&rect(1.0, 1.001, 0.0, 1.0), basis),
        level_face(&rect(1.001, 2.0, 0.0, 1.0), basis),
    ]);
    let drift = Vec3::new(3.0, 0.0, -1.5);
    let rate = parts[0].scroll_rate(drift);
    for part in &parts[1..] {
        assert_eq!(part.uv_gradient, parts[0].uv_gradient);
        assert_eq!(part.scroll_rate(drift), rate);
    }

    // A face on another plane, or with another projection, keeps its own map.
    let shifted = level_face(&rect(3.0, 4.0, 0.0, 1.0), projection(1.0, 0.0));
    let separate = maps(vec![level_face(&rect(0.0, 1.0, 0.0, 1.0), basis), shifted]);
    assert_ne!(separate[0].uv_gradient, separate[1].uv_gradient);
}

#[test]
fn drift_rates_fill_each_faces_vertices_and_a_mismatched_slice_leaves_all_still() {
    let basis = projection(1.0, 0.0);
    let mesh = pack_fluid_surfaces(
        &[record(vec![
            level_face(&rect(0.0, 1.0, 0.0, 1.0), basis),
            level_face(&rect(1.0, 2.0, 0.0, 1.0), basis),
        ])],
        &[DIMS],
        1,
    );
    let mut rates = vec![[9.0; 2]; mesh.vertices.len()];
    fill_drift_rates(
        &mesh.face_drift_maps,
        &[Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0)],
        &mut rates,
    );
    let second = mesh.face_drift_maps[1].unwrap();
    let start = second.vertex_start as usize;
    for (index, rate) in rates.iter().enumerate() {
        if (start..start + 4).contains(&index) {
            assert_eq!(
                *rate,
                second.scroll_rate(Vec3::new(3.0, 0.0, 0.0)).to_array()
            );
        } else {
            assert_eq!(*rate, [0.0; 2]);
        }
    }
    fill_drift_rates(&mesh.face_drift_maps, &[Vec3::X], &mut rates);
    assert!(rates.iter().all(|rate| *rate == [0.0; 2]));
}
