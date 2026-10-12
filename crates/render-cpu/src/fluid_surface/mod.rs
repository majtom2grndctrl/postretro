// Fluid surface data logic: environment-volume surface faces packed into one
// shared mesh, each frame's visible fluid draws, and per-volume draw slots.
// See: context/lib/rendering_pipeline.md §7.3a

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentVolumeRecord,
};
use postretro_level_format::geometry::NO_TEXTURE;

mod cull;
mod slots;

pub use cull::FluidSurfaceRenderCollector;
pub use slots::{FluidSurfaceSlots, slot_window};

/// One fluid surface vertex: world position, the face's outward normal, and
/// the texture UV normalized by the face texture's dimensions.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct FluidSurfaceVertex {
    pub position: [f32; 3],
    /// Outward unit normal from the face's CCW-from-outside winding (Newell).
    /// Flat per face; the shader flips it toward the viewer.
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// Byte stride of [`FluidSurfaceVertex`], mirrored by the pipeline's vertex
/// buffer layout and `fluid_surface.wgsl`'s `VertexInput`.
pub const FLUID_SURFACE_VERTEX_STRIDE: usize = 32;
const _: () = assert!(std::mem::size_of::<FluidSurfaceVertex>() == FLUID_SURFACE_VERTEX_STRIDE);

/// Per-volume parameters are one `vec4<f32>` per draw slot.
pub const FLUID_PARAMS_STRIDE: usize = 16;
/// Slots one uniform window holds: 64 KiB, the WebGPU default
/// `max_uniform_buffer_binding_size` and D3D's constant-buffer ceiling.
pub const FLUID_SLOTS_PER_WINDOW: u32 = 4096;
/// Byte size of one uniform window, which is also its dynamic-offset step.
pub const FLUID_PARAMS_WINDOW_BYTES: u64 =
    FLUID_SLOTS_PER_WINDOW as u64 * FLUID_PARAMS_STRIDE as u64;

/// One contiguous index range of one volume drawn with one world material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidMaterialRange {
    pub material_index: usize,
    pub index_start: u32,
    pub index_count: u32,
}

/// One volume that has drawable surface faces. Its position in
/// [`FluidSurfaceMesh::volumes`] is its draw slot.
#[derive(Debug, Clone, PartialEq)]
pub struct FluidSurfaceVolume {
    /// Index into `EnvironmentVolumesSection::volumes`.
    pub volume_index: u32,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    /// One range per material, in ascending material order.
    pub material_ranges: Vec<FluidMaterialRange>,
}

/// Every drawable fluid surface of a level in one shared vertex/index buffer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FluidSurfaceMesh {
    pub vertices: Vec<FluidSurfaceVertex>,
    pub indices: Vec<u32>,
    pub volumes: Vec<FluidSurfaceVolume>,
}

/// One fluid volume to draw this frame, at its fluid's surface opacity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidSurfaceDraw {
    pub volume_index: u32,
    pub surface_opacity: f32,
}

/// Outward unit normal of a convex face from its winding (Newell's method),
/// or `None` for a degenerate or non-finite face, which draws nothing.
pub fn face_normal(face: &EnvironmentSurfaceFace) -> Option<Vec3> {
    let vertices = &face.vertices;
    if vertices.len() < 3 {
        return None;
    }
    let mut normal = Vec3::ZERO;
    for (i, vertex) in vertices.iter().enumerate() {
        let a = Vec3::from(vertex.position);
        let b = Vec3::from(vertices[(i + 1) % vertices.len()].position);
        normal.x += (a.y - b.y) * (a.z + b.z);
        normal.y += (a.z - b.z) * (a.x + b.x);
        normal.z += (a.x - b.x) * (a.y + b.y);
    }
    // Newell's vector is twice the polygon's area; this floor drops slivers
    // far below the compiler's minimum kept piece.
    let length = normal.length();
    (length.is_finite() && length > 1.0e-9).then(|| normal / length)
}

/// World bounds over a volume's drawable surface faces; `None` when it has none.
pub fn surface_bounds(volume: &EnvironmentVolumeRecord) -> Option<(Vec3, Vec3)> {
    let mut bounds: Option<(Vec3, Vec3)> = None;
    for face in volume
        .surface_faces
        .iter()
        .filter(|face| face_normal(face).is_some())
    {
        for vertex in &face.vertices {
            let p = Vec3::from(vertex.position);
            bounds = Some(match bounds {
                Some((min, max)) => (min.min(p), max.max(p)),
                None => (p, p),
            });
        }
    }
    bounds
}

/// World material slot for a face texture: out-of-range or absent textures
/// take the placeholder at slot 0, as kinematic mover faces do.
pub fn material_index(texture_index: u32, material_count: usize) -> usize {
    if texture_index == NO_TEXTURE {
        return 0;
    }
    let index = texture_index as usize;
    if index < material_count { index } else { 0 }
}

fn normalized_uv(
    texel_uv: [f32; 2],
    texture_index: u32,
    texture_dimensions: &[[u32; 2]],
) -> [f32; 2] {
    // Surface UVs are texel-space, as world and mover UVs are before
    // `normalize_world_uvs`. Unknown dimensions leave them raw, as there.
    match texture_dimensions.get(texture_index as usize) {
        Some(&[width, height]) if width > 0 && height > 0 => {
            [texel_uv[0] / width as f32, texel_uv[1] / height as f32]
        }
        _ => texel_uv,
    }
}

/// Fan-triangulate every drawable surface face into one shared mesh. Each
/// volume's faces are grouped by material, so a volume draws one range per
/// material. Volumes without drawable faces get no slot.
pub fn pack_fluid_surfaces(
    volumes: &[EnvironmentVolumeRecord],
    texture_dimensions: &[[u32; 2]],
    material_count: usize,
) -> FluidSurfaceMesh {
    let mut mesh = FluidSurfaceMesh::default();
    let mut faces: Vec<(usize, Vec3, &EnvironmentSurfaceFace)> = Vec::new();
    for (volume_index, volume) in volumes.iter().enumerate() {
        faces.clear();
        faces.extend(volume.surface_faces.iter().filter_map(|face| {
            face_normal(face).map(|normal| {
                (
                    material_index(face.texture_index, material_count),
                    normal,
                    face,
                )
            })
        }));
        if faces.is_empty() {
            continue;
        }
        // Stable: faces keep compiled order within a material.
        faces.sort_by_key(|(material, _, _)| *material);

        let mut ranges: Vec<FluidMaterialRange> = Vec::new();
        let mut bounds_min = Vec3::splat(f32::INFINITY);
        let mut bounds_max = Vec3::splat(f32::NEG_INFINITY);
        for &(material, normal, face) in &faces {
            let base = mesh.vertices.len() as u32;
            let index_start = mesh.indices.len() as u32;
            for vertex in &face.vertices {
                let position = Vec3::from(vertex.position);
                bounds_min = bounds_min.min(position);
                bounds_max = bounds_max.max(position);
                mesh.vertices.push(FluidSurfaceVertex {
                    position: vertex.position,
                    normal: normal.to_array(),
                    uv: normalized_uv(vertex.uv, face.texture_index, texture_dimensions),
                });
            }
            for i in 1..face.vertices.len() as u32 - 1 {
                mesh.indices
                    .extend_from_slice(&[base, base + i, base + i + 1]);
            }
            let index_count = mesh.indices.len() as u32 - index_start;
            match ranges.last_mut() {
                Some(last) if last.material_index == material => last.index_count += index_count,
                _ => ranges.push(FluidMaterialRange {
                    material_index: material,
                    index_start,
                    index_count,
                }),
            }
        }
        mesh.volumes.push(FluidSurfaceVolume {
            volume_index: volume_index as u32,
            bounds_min,
            bounds_max,
            material_ranges: ranges,
        });
    }
    mesh
}

#[cfg(test)]
mod tests;
