// Fluid surface data logic: the shared surface mesh, drift, draws and slots,
// the reflection probe table, and the warp, fog and reflection CPU references.
// See: context/lib/rendering_pipeline.md §7.3a

use bytemuck::{Pod, Zeroable};
use glam::{DVec2, DVec3, Vec2, Vec3};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};
use postretro_level_format::geometry::NO_TEXTURE;

mod cull;
mod drift;
mod ear_clip;
mod ear_index;
pub mod fog;
pub mod grid;
pub mod motion;
pub mod reflection;
pub mod reflection_search;
pub mod reflection_style;
mod slots;

pub use cull::FluidSurfaceRenderCollector;
pub use drift::{
    FluidFaceDriftMap, UV_TOLERANCE_TEXELS as DRIFT_UV_TOLERANCE_TEXELS, fill_drift_rates,
};
pub use grid::{FLUID_GRID_VERTEX_CAP, FluidGridLattice, FluidGridPlanes};
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
    /// The planes the faces were cut along, `None` when packed ungridded.
    pub grid: Option<FluidGridPlanes>,
}

/// One fluid volume to draw this frame, with its fluid's packed look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidSurfaceDraw {
    pub volume_index: u32,
    pub params: FluidParamsRow,
}

/// Outward unit normal of a convex face from its winding (Newell's method),
/// or `None` for a degenerate or non-finite face, which draws nothing.
///
/// Summed in f64, relative to the first vertex. Each Newell term scales with
/// the positions' magnitude while the sum is twice the face's area, so in f32
/// on absolute positions a narrow face far from the origin, with thousands of
/// inserted vertices on one long edge, rounds past its own area and flips.
pub fn face_normal(face: &EnvironmentSurfaceFace) -> Option<Vec3> {
    let vertices = &face.vertices;
    if vertices.len() < 3 {
        return None;
    }
    let first = world_position(&vertices[0]);
    let mut normal = DVec3::ZERO;
    for (i, vertex) in vertices.iter().enumerate() {
        let a = world_position(vertex) - first;
        let b = world_position(&vertices[(i + 1) % vertices.len()]) - first;
        normal.x += (a.y - b.y) * (a.z + b.z);
        normal.y += (a.z - b.z) * (a.x + b.x);
        normal.z += (a.x - b.x) * (a.y + b.y);
    }
    // Newell's vector is twice the polygon's area; this floor drops slivers
    // far below the compiler's minimum kept piece.
    let length = normal.length();
    (length.is_finite() && length > 1.0e-9).then(|| (normal / length).as_vec3())
}

/// Area centroid of a face with outward unit `normal`, the face's
/// [`face_normal`]: the point the app samples its drift push from. The
/// compiler's T-junction inserts lie within 0.1 mm of an edge, so they add
/// next to no area, and thousands on one edge leave it where it was, where the
/// vertex mean would sit within a millimetre of that edge. Summed over the fan
/// from the first vertex in f64, relative to it, so it holds its precision far
/// from the origin; the signed sum is exact for any simple ring, star-shaped
/// or not. A face too thin to weigh, or whose `normal` disagrees with its
/// winding and so weighs the fan negative, takes its vertex mean.
pub fn face_area_centroid(face: &EnvironmentSurfaceFace, normal: Vec3) -> DVec3 {
    let normal = normal.as_dvec3();
    let first = world_position(&face.vertices[0]);
    let relative = |i: usize| world_position(&face.vertices[i]) - first;
    let (mut weighted, mut weight) = (DVec3::ZERO, 0.0);
    for i in 1..face.vertices.len().saturating_sub(1) {
        let (a, b) = (relative(i), relative(i + 1));
        let twice_area = a.cross(b).dot(normal);
        weighted += twice_area * (a + b) / 3.0;
        weight += twice_area;
    }
    if weight > 0.0 && weighted.is_finite() {
        return first + weighted / weight;
    }
    let sum: DVec3 = (0..face.vertices.len()).map(relative).sum();
    first + sum / face.vertices.len() as f64
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
/// inserts a T-junction vertex within 0.1 mm of its edge's line, judged on
/// the encoded f32 positions, so their rounding is already inside that. Twice
/// it covers an insert judged against an edge that earlier inserts or welds
/// bent, which can leave it a little over 0.1 mm from its final neighbours'
/// chord. Absolute, as the compiler's vertex spacing is: vertices 0.1 mm
/// apart are as near on a 10 m face as on a 10 cm one, and an insert 0.1 mm
/// off its edge dents a 6 mm face as it does a 6 m one. A sliver, or a face
/// under a millimetre across, can hold real corners within it; the ear clip
/// tiles such a convex ring over the same vertices, as the fan would.
const COLLINEAR_DISTANCE: f64 = 2.0e-4;

fn world_position(vertex: &EnvironmentSurfaceVertex) -> DVec3 {
    Vec3::from(vertex.position).as_dvec3()
}

// Steps of the per-face install work that reads a face's vertices: the drift
// fit and fit test, the grid split, the ear clip and the plain fan. Each
// counted unit is O(1), or O(log n) for an ordered-set operation. Closure can
// give one face thousands of vertices, so tests bound this count's growth,
// not wall time.
#[cfg(test)]
thread_local! {
    static INSTALL_STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn count_install_steps(steps: usize) {
    INSTALL_STEPS.with(|count| count.set(count.get() + steps as u64));
}

#[cfg(not(test))]
fn count_install_steps(_steps: usize) {}

/// Whether a face has a vertex on, or within [`COLLINEAR_DISTANCE`]
/// of, the line through its ring neighbours, or one (nearly) repeating the
/// vertex before it.
fn has_collinear_vertex(vertices: &[EnvironmentSurfaceVertex]) -> bool {
    let count = vertices.len();
    let at = |i: usize| world_position(&vertices[i % count]);
    (0..count).any(|i| {
        let (a, p, c) = (at(i + count - 1), at(i), at(i + 1));
        let chord = c - a;
        let length = chord.length();
        p.distance(a) <= COLLINEAR_DISTANCE
            || length <= COLLINEAR_DISTANCE
            || (p - a).cross(chord).length() / length <= COLLINEAR_DISTANCE
    })
}

/// Triangulate one convex face, packed from mesh vertex `base` in ring order,
/// appending indices wound CCW about its outward unit `normal`.
///
/// The surface is translucent, so a T-junction shows as a crack. The compiler
/// closes them by inserting vertices on face edges, each within 0.1 mm of the
/// edge it joins, at its own absolute spacing (down to 0.1 mm, plus duplicates
/// after f32 rounding). A fan from one apex cannot honour such a vertex: the
/// triangle beside it collapses and a chord spans an edge across it. Nor can a
/// fan from an added centroid vertex: the face stays simple and within about
/// 0.1 mm of convex, but need not be star-shaped about its centroid. On a
/// thin strip, an insert a hair inside a long edge near a corner lies past
/// the short edge's ray from the centroid, and the fan triangle beside it
/// inverts. Nor can the plain fan honour an insert that dents its edge: the
/// fan triangle from a neighbouring corner over it winds backward. A face with
/// a collinear vertex therefore ear-clips over its own vertices, as a grid
/// part does ([`ear_clip::triangulate_convex`]), adding none: each vertex stays a
/// corner except one repeating the vertex before it, which joins the ring
/// once, so each segment of non-zero length borders one triangle; no ear
/// holds a dent, so the face keeps its area exactly. Taking a near-flat real
/// corner for collinear is harmless. Every other face keeps the fan from its
/// first vertex: `n - 2` triangles.
fn triangulate_convex_face(
    face: &EnvironmentSurfaceFace,
    normal: Vec3,
    base: u32,
    indices: &mut Vec<u32>,
) {
    let count = face.vertices.len() as u32;
    count_install_steps(face.vertices.len());
    if !has_collinear_vertex(&face.vertices) {
        for i in 1..count - 1 {
            indices.extend_from_slice(&[base, base + i, base + i + 1]);
        }
        return;
    }
    let positions: Vec<DVec3> = face.vertices.iter().map(world_position).collect();
    let ring: Vec<u32> = (0..count).collect();
    let mut triangles = Vec::with_capacity(face.vertices.len().saturating_sub(2));
    ear_clip::triangulate_convex(&ring, &positions, normal.as_dvec3(), &mut triangles);
    indices.extend(
        triangles
            .iter()
            .flat_map(|triangle| triangle.map(|vertex| base + vertex)),
    );
}

/// Triangulate every drawable surface face into one shared mesh. With a
/// usable `lattice`, every face is first cut along the SH lattice's planes
/// (`grid`), so baked light evaluated per vertex varies within a probe cell;
/// the level's vertex count stays within [`FLUID_GRID_VERTEX_CAP`]. A face no
/// plane crosses, or every face without a grid, packs its own vertices alone:
/// a face with a collinear vertex ear-clips over them, so the vertices that
/// close T-junctions stay triangle corners; every other face is the fan from
/// its first vertex. Each face's mesh vertices stay contiguous.
/// Each volume's faces are grouped by material, so a volume draws one range
/// per material. Volumes without drawable faces get no slot.
pub fn pack_fluid_surfaces(
    volumes: &[EnvironmentVolumeRecord],
    texture_dimensions: &[[u32; 2]],
    material_count: usize,
    lattice: Option<&FluidGridLattice>,
) -> FluidSurfaceMesh {
    pack_fluid_surfaces_within(
        volumes,
        texture_dimensions,
        material_count,
        lattice,
        FLUID_GRID_VERTEX_CAP,
    )
}

fn pack_fluid_surfaces_within(
    volumes: &[EnvironmentVolumeRecord],
    texture_dimensions: &[[u32; 2]],
    material_count: usize,
    lattice: Option<&FluidGridLattice>,
    vertex_cap: u64,
) -> FluidSurfaceMesh {
    let mut mesh = FluidSurfaceMesh {
        grid: grid::choose_grid_planes(volumes, lattice, vertex_cap),
        ..FluidSurfaceMesh::default()
    };
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
            }
            let uvs: Vec<[f32; 2]> = face
                .vertices
                .iter()
                .map(|vertex| normalized_uv(vertex.uv, face.texture_index, texture_dimensions))
                .collect();
            let cut = mesh.grid.as_ref().and_then(|planes| {
                let uvs: Vec<DVec2> = uvs.iter().map(|&uv| Vec2::from(uv).as_dvec2()).collect();
                grid::cut_face(face, normal, &uvs, planes)
            });
            match cut {
                Some(cut) => {
                    // The face's own vertices lead, so they round-trip unchanged.
                    for (position, uv) in cut.positions.iter().zip(&cut.uvs) {
                        mesh.vertices.push(FluidSurfaceVertex {
                            position: position.as_vec3().to_array(),
                            normal: normal.to_array(),
                            uv: uv.as_vec2().to_array(),
                        });
                    }
                    mesh.indices.extend(
                        cut.triangles
                            .iter()
                            .flat_map(|triangle| triangle.map(|vertex| base + vertex)),
                    );
                }
                None => {
                    for (vertex, uv) in face.vertices.iter().zip(uvs) {
                        mesh.vertices.push(FluidSurfaceVertex {
                            position: vertex.position,
                            normal: normal.to_array(),
                            uv,
                        });
                    }
                    triangulate_convex_face(face, normal, base, &mut mesh.indices);
                }
            }
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
