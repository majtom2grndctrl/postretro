// Fluid surface data logic: surface faces packed into one shared mesh, per-face
// drift maps and rates, visible draws, draw slots and their parameter rows, the
// reflection probe table, and the warp's, depth fog's, reflection's and
// reflection style's CPU references.
// See: context/lib/rendering_pipeline.md §7.3a

use bytemuck::{Pod, Zeroable};
use glam::{DVec2, DVec3, Vec2, Vec3};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};
use postretro_level_format::geometry::NO_TEXTURE;

mod cull;
mod drift;
pub mod fog;
pub mod motion;
pub mod reflection;
pub mod reflection_search;
pub mod reflection_style;
mod slots;

pub use cull::FluidSurfaceRenderCollector;
pub use drift::{FluidFaceDriftMap, fill_drift_rates};
pub use slots::{
    FLUID_FOG_COLOR_SCALE, FluidParamsRow, FluidShaderVariant, FluidSurfaceLook, FluidSurfaceSlots,
    slot_window,
};

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

/// Byte stride of one per-vertex drift rate (`[f32; 2]`, texture periods per
/// second) in the second vertex buffer, which drift re-resolves rewrite.
pub const FLUID_DRIFT_RATE_STRIDE: usize = 8;

/// Byte stride of one per-vertex probe index (`u32`) in the third vertex
/// buffer, written once at install (`reflection::fill_probe_indices`).
pub const FLUID_PROBE_INDEX_STRIDE: usize = 4;

/// Per-volume parameters are three `vec4<f32>` per draw slot ([`FluidParamsRow`]).
pub const FLUID_PARAMS_STRIDE: usize = 48;
const _: () = assert!(std::mem::size_of::<FluidParamsRow>() == FLUID_PARAMS_STRIDE);
/// Slots one uniform window holds. 1365 rows would fill 64 KiB (the WebGPU
/// default `max_uniform_buffer_binding_size` and D3D's constant-buffer
/// ceiling), but 65 520 B is no multiple of the 256 B dynamic-offset
/// alignment; 1360 rows are 65 280 B = 255 × 256, so every window's offset is.
pub const FLUID_SLOTS_PER_WINDOW: u32 = 1360;
/// Byte size of one uniform window, which is also its dynamic-offset step.
pub const FLUID_PARAMS_WINDOW_BYTES: u64 =
    FLUID_SLOTS_PER_WINDOW as u64 * FLUID_PARAMS_STRIDE as u64;
const _: () = assert!(
    FLUID_PARAMS_WINDOW_BYTES.is_multiple_of(256) && FLUID_PARAMS_WINDOW_BYTES <= 65_536,
    "a params window must be a 256 B dynamic-offset step within 64 KiB"
);

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
    /// One entry per surface face of every volume, in record order (volume,
    /// then face), `None` for a face that draws nothing. A drift slice in the
    /// same order turns into per-vertex rates ([`fill_drift_rates`]).
    pub face_drift_maps: Vec<Option<FluidFaceDriftMap>>,
}

/// One fluid volume to draw this frame, with its fluid's packed look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidSurfaceDraw {
    pub volume_index: u32,
    pub params: FluidParamsRow,
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

/// Texels per normalized UV unit for a face texture.
fn uv_scale(texture_index: u32, texture_dimensions: &[[u32; 2]]) -> Vec2 {
    // Surface UVs are texel-space, as world and mover UVs are before
    // `normalize_world_uvs`. Unknown dimensions leave them raw, as there.
    match texture_dimensions.get(texture_index as usize) {
        Some(&[width, height]) if width > 0 && height > 0 => Vec2::new(width as f32, height as f32),
        _ => Vec2::ONE,
    }
}

fn normalized_uv(
    texel_uv: [f32; 2],
    texture_index: u32,
    texture_dimensions: &[[u32; 2]],
) -> [f32; 2] {
    (Vec2::from(texel_uv) / uv_scale(texture_index, texture_dimensions)).to_array()
}

/// A vertex within this distance, meters, of the line through its ring
/// neighbours, or of the neighbour before it, is collinear. The compiler
/// inserts a T-junction vertex within 0.1 mm of its edge's line; twice that
/// also covers f32 position rounding within about 1 km of the origin.
/// Absolute, as the compiler's vertex spacing is: vertices 0.1 mm apart are
/// as near on a 10 m face as on a 10 cm one.
const COLLINEAR_DISTANCE: f64 = 2.0e-4;
/// A face under 2 cm across takes this fraction of its extent instead, so its
/// real corners keep the plain fan.
const COLLINEAR_EXTENT_FRACTION: f64 = 1.0e-2;

fn world_position(vertex: &EnvironmentSurfaceVertex) -> DVec3 {
    Vec3::from(vertex.position).as_dvec3()
}

/// Whether a convex face has a vertex on, or within [`COLLINEAR_DISTANCE`]
/// of, the line through its ring neighbours, or one (nearly) repeating the
/// vertex before it.
fn has_collinear_vertex(vertices: &[EnvironmentSurfaceVertex]) -> bool {
    let count = vertices.len();
    let at = |i: usize| world_position(&vertices[i % count]);
    let origin = at(0);
    let extent = vertices
        .iter()
        .map(|vertex| world_position(vertex).distance(origin))
        .fold(0.0, f64::max);
    let tolerance = COLLINEAR_DISTANCE.min(COLLINEAR_EXTENT_FRACTION * extent);
    (0..count).any(|i| {
        let (a, p, c) = (at(i + count - 1), at(i), at(i + 1));
        let chord = c - a;
        let length = chord.length();
        p.distance(a) <= tolerance
            || length <= tolerance
            || (p - a).cross(chord).length() / length <= tolerance
    })
}

/// Triangulate one convex face whose vertices start at `base` in `vertices`,
/// appending indices with the face's CCW-from-outside winding.
///
/// The surface is translucent, so a T-junction shows as a crack. In a volume a
/// push cut, the compiler closes them by inserting collinear vertices on face
/// edges, at its own absolute spacing (down to 0.1 mm, plus duplicates after
/// f32 rounding). A fan from one apex cannot honour such a vertex: the
/// triangle beside it collapses and a chord spans an edge across it. A face
/// with a collinear vertex therefore gains one vertex at its vertex centroid,
/// which lies inside the convex face, and fans from it over every boundary
/// segment. Every boundary vertex stays a corner, every segment borders one
/// triangle, and no area is dropped, whatever the spacing; only a zero-length
/// segment draws nothing. Taking a near-flat real corner for collinear is
/// harmless. Every other face keeps the fan from its first vertex: `n - 2`
/// triangles and no extra vertex.
fn triangulate_convex_face(
    face: &EnvironmentSurfaceFace,
    base: u32,
    vertices: &mut Vec<FluidSurfaceVertex>,
    indices: &mut Vec<u32>,
) {
    let count = face.vertices.len() as u32;
    if !has_collinear_vertex(&face.vertices) {
        for i in 1..count - 1 {
            indices.extend_from_slice(&[base, base + i, base + i + 1]);
        }
        return;
    }
    let ring = &vertices[base as usize..];
    let mut position_sum = DVec3::ZERO;
    let mut uv_sum = DVec2::ZERO;
    for vertex in ring {
        position_sum += Vec3::from(vertex.position).as_dvec3();
        uv_sum += Vec2::from(vertex.uv).as_dvec2();
    }
    let inverse = 1.0 / f64::from(count);
    // UV is affine in position, so the mean UV is the centroid's exact UV.
    let centroid = FluidSurfaceVertex {
        position: (position_sum * inverse).as_vec3().to_array(),
        normal: ring[0].normal,
        uv: (uv_sum * inverse).as_vec2().to_array(),
    };
    vertices.push(centroid);
    let center = base + count;
    for i in 0..count {
        let (a, b) = (base + i, base + (i + 1) % count);
        if vertices[a as usize].position != vertices[b as usize].position {
            indices.extend_from_slice(&[center, a, b]);
        }
    }
}

/// Triangulate every drawable surface face into one shared mesh. A face with
/// a collinear vertex fans from an added centroid vertex, so the vertices
/// that close T-junctions stay triangle corners; every other face is the fan
/// from its first vertex. Each face's mesh vertices stay contiguous. Each
/// volume's faces are grouped by material, so a volume draws one range per
/// material. Volumes without drawable faces get no slot.
pub fn pack_fluid_surfaces(
    volumes: &[EnvironmentVolumeRecord],
    texture_dimensions: &[[u32; 2]],
    material_count: usize,
) -> FluidSurfaceMesh {
    let mut mesh = FluidSurfaceMesh::default();
    let mut faces: Vec<(usize, Vec3, usize)> = Vec::new();
    // Per surface face: its first mesh vertex and its mesh vertex count.
    let mut vertex_ranges: Vec<Option<(u32, u32)>> = Vec::new();
    for (volume_index, volume) in volumes.iter().enumerate() {
        faces.clear();
        faces.extend(
            volume
                .surface_faces
                .iter()
                .enumerate()
                .filter_map(|(face_index, face)| {
                    face_normal(face).map(|normal| {
                        (
                            material_index(face.texture_index, material_count),
                            normal,
                            face_index,
                        )
                    })
                }),
        );
        vertex_ranges.clear();
        vertex_ranges.resize(volume.surface_faces.len(), None);
        if faces.is_empty() {
            mesh.face_drift_maps
                .extend(std::iter::repeat_n(None, volume.surface_faces.len()));
            continue;
        }
        // Stable: faces keep compiled order within a material.
        faces.sort_by_key(|(material, _, _)| *material);

        let mut ranges: Vec<FluidMaterialRange> = Vec::new();
        let mut bounds_min = Vec3::splat(f32::INFINITY);
        let mut bounds_max = Vec3::splat(f32::NEG_INFINITY);
        for &(material, normal, face_index) in &faces {
            let face = &volume.surface_faces[face_index];
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
            triangulate_convex_face(face, base, &mut mesh.vertices, &mut mesh.indices);
            vertex_ranges[face_index] = Some((base, mesh.vertices.len() as u32 - base));
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
        mesh.face_drift_maps.extend(drift::volume_drift_maps(
            volume,
            &vertex_ranges,
            |face, vertex| {
                Vec2::from(normalized_uv(
                    face.vertices[vertex].uv,
                    face.texture_index,
                    texture_dimensions,
                ))
            },
            |face| uv_scale(face.texture_index, texture_dimensions),
        ));
    }
    mesh
}

#[cfg(test)]
mod tests;
