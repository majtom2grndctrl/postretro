// World draw geometry: the vertex, index and wireframe buffers, and the BVH
// cull owners built over the set's own index buffer.
// See: context/lib/rendering_pipeline.md §5

use super::super::install_pipelines::InstallPipelines;
use super::*;
use crate::render::gpu_create::GpuCreate;

/// One level's world draw buffers.
pub(super) struct WorldBuffers {
    pub(super) vertex_buffer: wgpu::Buffer,
    pub(super) spec_owner_buffer: wgpu::Buffer,
    pub(super) index_buffer: wgpu::Buffer,
    pub(super) index_count: u32,
    pub(super) wireframe_index_buffer: wgpu::Buffer,
    pub(super) wireframe_index_count: u32,
    /// Whether the level has any geometry.
    pub(super) has_geometry: bool,
}

/// One level's BVH leaves and the cull owners over its index buffer.
pub(super) struct WorldCull {
    pub(super) bvh_leaves: Vec<postretro_render_data::geometry::BvhLeaf>,
    pub(super) cell_draw_index: Option<postretro_level_loader::CellDrawIndex>,
    pub(super) compute_cull: Option<ComputeCullPipeline>,
    pub(super) candidate_cull: Option<crate::candidate_cull::CandidateCullPipeline>,
}

/// Upload the level's vertex, spec-owner, index and wireframe buffers.
pub(super) fn install_world_buffers(
    device: &wgpu::Device,
    geometry: &LevelGeometry<'_>,
) -> WorldBuffers {
    let has_geometry = !geometry.vertices.is_empty() && !geometry.indices.is_empty();

    // --- Vertex / index buffers ---
    let (vertex_data, index_data, index_count) = if has_geometry {
        let count = geometry.indices.len() as u32;
        (
            cast_world_vertices_to_bytes(geometry.vertices),
            bytemuck_cast_slice_u32(geometry.indices),
            count,
        )
    } else {
        (
            vec![0u8; postretro_render_data::geometry::WorldVertex::STRIDE],
            vec![0u8; 4],
            0u32,
        )
    };
    let vertex_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("World Vertex Buffer"),
        contents: &vertex_data,
        usage: wgpu::BufferUsages::VERTEX,
    });
    // The forward pass's owner stream, written once here and never per frame.
    let spec_owner_data =
        cast_spec_owners_to_bytes(geometry.vertex_spec_owners, geometry.vertices.len());
    let spec_owner_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("World Spec Owner Buffer"),
        contents: &spec_owner_data,
        usage: wgpu::BufferUsages::VERTEX,
    });
    // Indirect slots copy only these checked leaves' baked ranges, and all
    // world indirect passes bind this complete array. Every cull owner is
    // built over this set's index buffer, never another set's.
    let index_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("World Index Buffer"),
        contents: &index_data,
        usage: wgpu::BufferUsages::INDEX,
    });

    // --- Wireframe index buffer ---
    let (wireframe_index_data, wireframe_index_count) = if has_geometry {
        let line_indices = build_line_indices_from_triangles(geometry.indices);
        let count = line_indices.len() as u32;
        (bytemuck_cast_slice_u32(&line_indices), count)
    } else {
        (vec![0u8; 4], 0u32)
    };
    let wireframe_index_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Wireframe Line Index Buffer"),
        contents: &wireframe_index_data,
        usage: wgpu::BufferUsages::INDEX,
    });
    WorldBuffers {
        vertex_buffer,
        spec_owner_buffer,
        index_buffer,
        index_count,
        wireframe_index_buffer,
        wireframe_index_count,
        has_geometry,
    }
}

pub(super) fn install_bvh_and_cull(
    device: &wgpu::Device,
    pipelines: &InstallPipelines,
    geometry: &LevelGeometry<'_>,
    bvh_leaves: Vec<postretro_render_data::geometry::BvhLeaf>,
    has_multi_draw_indirect: bool,
) -> WorldCull {
    // --- BVH + compute cull ---
    // Per-cell draw index for the candidate-cull path, cloned alongside the
    // BVH leaves.
    let cell_draw_index = geometry.cell_draw_index.cloned();
    let compute_cull = if !bvh_leaves.is_empty() {
        Some(ComputeCullPipeline::new(
            device,
            &pipelines.compute_cull,
            geometry.bvh,
            has_multi_draw_indirect,
        ))
    } else {
        None
    };
    // Rebuild the candidate-cull path in lockstep with `compute_cull`, sized
    // to the freshly-installed leaf count; an empty BVH builds neither.
    let candidate_cull = compute_cull.as_ref().map(|c| {
        crate::candidate_cull::CandidateCullPipeline::new(
            device,
            &pipelines.candidate_cull,
            c.total_leaves(),
        )
    });
    WorldCull {
        bvh_leaves,
        cell_draw_index,
        compute_cull,
        candidate_cull,
    }
}
