// Spatial grid test fixtures: helpers for spatial_grid tests.
// See: context/lib/testing_guide.md §4

use glam::Vec3;

use crate::map_data::Face;

/// Create a test face with the given vertices and a default Z normal.
pub fn make_face(vertices: Vec<Vec3>) -> Face {
    let normal = Vec3::Z;
    Face {
        vertices,
        normal,
        distance: 0.0,
        texture: "test".to_string(),
    }
}

/// Create a small triangle face centered at the given point.
pub fn make_triangle(center: Vec3) -> Face {
    make_face(vec![
        center + Vec3::new(-1.0, -1.0, 0.0),
        center + Vec3::new(1.0, -1.0, 0.0),
        center + Vec3::new(0.0, 1.0, 0.0),
    ])
}

/// Create the 6 faces of a box brush for the face_centroid_within_assigned_cell_bounds test.
pub fn make_box_for_centroid_test(min: Vec3, max: Vec3) -> Vec<Face> {
    vec![
        Face {
            vertices: vec![
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(min.x, max.y, min.z),
                Vec3::new(min.x, max.y, max.z),
                Vec3::new(min.x, min.y, max.z),
            ],
            normal: Vec3::NEG_X,
            distance: -min.x,
            texture: "test".to_string(),
        },
        Face {
            vertices: vec![
                Vec3::new(max.x, min.y, min.z),
                Vec3::new(max.x, min.y, max.z),
                Vec3::new(max.x, max.y, max.z),
                Vec3::new(max.x, max.y, min.z),
            ],
            normal: Vec3::X,
            distance: max.x,
            texture: "test".to_string(),
        },
        Face {
            vertices: vec![
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(min.x, min.y, max.z),
                Vec3::new(max.x, min.y, max.z),
                Vec3::new(max.x, min.y, min.z),
            ],
            normal: Vec3::NEG_Y,
            distance: -min.y,
            texture: "test".to_string(),
        },
        Face {
            vertices: vec![
                Vec3::new(min.x, max.y, min.z),
                Vec3::new(max.x, max.y, min.z),
                Vec3::new(max.x, max.y, max.z),
                Vec3::new(min.x, max.y, max.z),
            ],
            normal: Vec3::Y,
            distance: max.y,
            texture: "test".to_string(),
        },
        Face {
            vertices: vec![
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(max.x, min.y, min.z),
                Vec3::new(max.x, max.y, min.z),
                Vec3::new(min.x, max.y, min.z),
            ],
            normal: Vec3::NEG_Z,
            distance: -min.z,
            texture: "test".to_string(),
        },
        Face {
            vertices: vec![
                Vec3::new(min.x, min.y, max.z),
                Vec3::new(max.x, min.y, max.z),
                Vec3::new(max.x, max.y, max.z),
                Vec3::new(min.x, max.y, max.z),
            ],
            normal: Vec3::Z,
            distance: max.z,
            texture: "test".to_string(),
        },
    ]
}
