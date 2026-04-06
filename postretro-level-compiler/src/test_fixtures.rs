// Shared test fixtures: geometry builders used by multiple test modules.
// See: context/lib/testing_guide.md §4

use glam::Vec3;

use crate::map_data::{BrushPlane, BrushVolume, Face};
use crate::partition::Aabb;
use crate::voxel_grid::VoxelGrid;

/// Generate the 6 outward-facing faces of an axis-aligned box brush.
pub fn make_box_faces(min: Vec3, max: Vec3) -> Vec<Face> {
    let texture = "test".to_string();
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
            texture: texture.clone(),
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
            texture: texture.clone(),
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
            texture: texture.clone(),
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
            texture: texture.clone(),
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
            texture: texture.clone(),
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
            texture: texture.clone(),
        },
    ]
}

/// Build an axis-aligned box BrushVolume from min/max corners.
pub fn box_brush(min: Vec3, max: Vec3) -> BrushVolume {
    BrushVolume {
        planes: vec![
            BrushPlane {
                normal: Vec3::X,
                distance: max.x,
            },
            BrushPlane {
                normal: Vec3::NEG_X,
                distance: -min.x,
            },
            BrushPlane {
                normal: Vec3::Y,
                distance: max.y,
            },
            BrushPlane {
                normal: Vec3::NEG_Y,
                distance: -min.y,
            },
            BrushPlane {
                normal: Vec3::Z,
                distance: max.z,
            },
            BrushPlane {
                normal: Vec3::NEG_Z,
                distance: -min.z,
            },
        ],
    }
}

/// Generate the BrushVolume for an axis-aligned box brush.
///
/// Same geometry as `box_brush` but with normals ordered as negative-axis
/// first (matching the convention in visibility tests).
pub fn make_box_brush_volume(min: Vec3, max: Vec3) -> BrushVolume {
    BrushVolume {
        planes: vec![
            BrushPlane {
                normal: Vec3::NEG_X,
                distance: -min.x,
            },
            BrushPlane {
                normal: Vec3::X,
                distance: max.x,
            },
            BrushPlane {
                normal: Vec3::NEG_Y,
                distance: -min.y,
            },
            BrushPlane {
                normal: Vec3::Y,
                distance: max.y,
            },
            BrushPlane {
                normal: Vec3::NEG_Z,
                distance: -min.z,
            },
            BrushPlane {
                normal: Vec3::Z,
                distance: max.z,
            },
        ],
    }
}

/// Build a VoxelGrid from faces and brush volumes (no cluster bounds).
pub fn build_voxel_grid_from_faces(
    faces: &[Face],
    brush_volumes: &[BrushVolume],
) -> VoxelGrid {
    let mut world_bounds = Aabb::empty();
    for face in faces {
        for &v in &face.vertices {
            world_bounds.expand_point(v);
        }
    }
    if !world_bounds.is_valid() {
        world_bounds = Aabb {
            min: Vec3::ZERO,
            max: Vec3::splat(1.0),
        };
    }
    let pad = Vec3::splat(crate::voxel_grid::DEFAULT_VOXEL_SIZE);
    world_bounds.min -= pad;
    world_bounds.max += pad;
    VoxelGrid::from_brushes(
        brush_volumes,
        &world_bounds,
        crate::voxel_grid::DEFAULT_VOXEL_SIZE,
    )
}
