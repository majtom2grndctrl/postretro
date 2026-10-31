// Shared SH bake test fixtures: geometry, BSP trees, and lights.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::{DVec3, Vec3};
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::map_data::{FalloffModel, LightType, MapLight};
use crate::partition::{Aabb as CompilerAabb, BspLeaf, BspTree};

pub(super) fn point_light_with_falloff(model: FalloffModel, range: f32) -> MapLight {
    MapLight {
        origin: glam::DVec3::ZERO,
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: model,
        falloff_range: range,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: vec![],
        shadow_type: crate::map_data::ShadowType::StaticLightMap,
    }
}

fn tri_vertex(pos: [f32; 3]) -> Vertex {
    Vertex::new(
        pos,
        [0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    )
}

pub(super) fn one_triangle_geometry(positions: [[f32; 3]; 3]) -> GeometryResult {
    GeometryResult {
        geometry: GeometrySection {
            vertices: positions.iter().map(|&p| tri_vertex(p)).collect(),
            indices: vec![0, 1, 2],
            faces: vec![FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            }],
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: vec![FaceIndexRange {
            index_offset: 0,
            index_count: 3,
        }],
    }
}

/// Geometry from a list of triangles, one BVH face/primitive per triangle.
/// A single floor triangle gives near-uniform ray distances (most rays sky-
/// miss to the sentinel, the rest graze one plane); multiple triangles at
/// different depths give the depth-moment accumulator a real spread of
/// `closest_hit` distances to integrate.
pub(super) fn multi_triangle_geometry(triangles: &[[[f32; 3]; 3]]) -> GeometryResult {
    let mut vertices = Vec::with_capacity(triangles.len() * 3);
    let mut indices = Vec::with_capacity(triangles.len() * 3);
    let mut faces = Vec::with_capacity(triangles.len());
    let mut face_index_ranges = Vec::with_capacity(triangles.len());
    for (i, tri) in triangles.iter().enumerate() {
        let base = (i * 3) as u32;
        for &p in tri {
            vertices.push(tri_vertex(p));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2]);
        faces.push(FaceMeta {
            leaf_index: 0,
            texture_index: 0,
        });
        face_index_ranges.push(FaceIndexRange {
            index_offset: base,
            index_count: 3,
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

/// A 4 m floor plus three walls forming an open-topped box. Probes in the
/// box see a real spread of `closest_hit` distances: rays toward a near wall
/// hit close, rays toward a far wall hit far, rays out the open top sky-miss
/// to the sentinel. Exercises the depth-moment accumulation that a single
/// floor triangle leaves near-degenerate.
pub(super) fn floor_and_walls_geometry() -> GeometryResult {
    // Floor (two triangles) spanning x,z in [0, 4] at y = 0.
    let floor_a = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 0.0, 4.0]];
    let floor_b = [[0.0, 0.0, 0.0], [4.0, 0.0, 4.0], [0.0, 0.0, 4.0]];
    // Near wall at x = 0 and far wall at x = 4, plus a side wall at z = 4,
    // each a single triangle rising to y = 3. Their differing distances from
    // any interior probe spread the per-ray hit distances.
    let wall_near = [[0.0, 0.0, 0.0], [0.0, 0.0, 4.0], [0.0, 3.0, 0.0]];
    let wall_far = [[4.0, 0.0, 0.0], [4.0, 0.0, 4.0], [4.0, 3.0, 0.0]];
    let wall_side = [[0.0, 0.0, 4.0], [4.0, 0.0, 4.0], [0.0, 3.0, 4.0]];
    multi_triangle_geometry(&[floor_a, floor_b, wall_near, wall_far, wall_side])
}

pub(super) fn tree_all_empty() -> BspTree {
    BspTree {
        nodes: Vec::new(),
        leaves: vec![BspLeaf {
            face_indices: Vec::new(),
            bounds: CompilerAabb {
                min: DVec3::splat(-1000.0),
                max: DVec3::splat(1000.0),
            },
            is_solid: false,
            defining_planes: Vec::new(),
        }],
    }
}

/// Normals at `cone_deg` off `l`, swept over four azimuths around `l`, so a
/// per-azimuth or single-band normalization error can't hide behind one sample.
pub(super) fn normals_on_cone(l: Vec3, cone_deg: f32) -> Vec<Vec3> {
    // Pick a reference not parallel to `l` to build a stable tangent frame.
    let up = if l.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let tangent = l.cross(up).normalize();
    let bitangent = l.cross(tangent).normalize();
    let a = cone_deg.to_radians();
    [0.0f32, 90.0, 180.0, 270.0]
        .iter()
        .map(|&phi_deg| {
            let phi = phi_deg.to_radians();
            let radial = tangent * phi.cos() + bitangent * phi.sin();
            (l * a.cos() + radial * a.sin()).normalize()
        })
        .collect()
}

pub(super) fn point_light_colored(
    origin: glam::DVec3,
    color: [f32; 3],
    intensity: f32,
) -> MapLight {
    MapLight {
        origin,
        carrier: String::new(),
        light_type: LightType::Point,
        intensity,
        color,
        falloff_model: FalloffModel::Linear,
        falloff_range: 20.0,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: vec![],
        shadow_type: crate::map_data::ShadowType::StaticLightMap,
    }
}

/// Soft-shadow shadow-casting point light at `origin` with the given world-unit
/// area radius. `light_size > 0` takes the soft area-sample path in
/// `soft_visibility`; `0` collapses to the single hard ray.
pub(super) fn soft_point_light(origin: DVec3, light_size: f32) -> MapLight {
    MapLight {
        origin,
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 50.0,
        light_size,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: vec![],
        shadow_type: crate::map_data::ShadowType::StaticLightMap,
    }
}
