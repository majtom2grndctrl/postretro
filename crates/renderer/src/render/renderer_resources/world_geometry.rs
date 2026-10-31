// World draw geometry: the vertex, index and wireframe buffers, and the BVH
// cull owners recreated over the index buffer they address.
// See: context/lib/rendering_pipeline.md §5

use super::super::renderer_types::FullRenderer;
use super::*;
use crate::render::gpu_create::GpuCreate;

/// Upload the level's vertex, spec-owner, index and wireframe buffers.
/// Returns whether the level has any geometry.
pub(super) fn install_world_buffers(
    device: &wgpu::Device,
    full: &mut FullRenderer,
    geometry: &LevelGeometry<'_>,
) -> bool {
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
    full.vertex_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("World Vertex Buffer"),
        contents: &vertex_data,
        usage: wgpu::BufferUsages::VERTEX,
    });
    // The forward pass's owner stream, written once here and never per frame.
    let spec_owner_data =
        cast_spec_owners_to_bytes(geometry.vertex_spec_owners, geometry.vertices.len());
    full.spec_owner_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("World Spec Owner Buffer"),
        contents: &spec_owner_data,
        usage: wgpu::BufferUsages::VERTEX,
    });
    // Indirect slots copy only these checked leaves' baked ranges, and all
    // world indirect passes bind this complete array. Recreate every cull
    // owner below before drawing; no slot may survive an index-buffer swap.
    full.index_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("World Index Buffer"),
        contents: &index_data,
        usage: wgpu::BufferUsages::INDEX,
    });
    full.index_count = index_count;

    // --- Wireframe index buffer ---
    let (wireframe_index_data, wireframe_index_count) = if has_geometry {
        let line_indices = build_line_indices_from_triangles(geometry.indices);
        let count = line_indices.len() as u32;
        (bytemuck_cast_slice_u32(&line_indices), count)
    } else {
        (vec![0u8; 4], 0u32)
    };
    full.wireframe_index_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Wireframe Line Index Buffer"),
        contents: &wireframe_index_data,
        usage: wgpu::BufferUsages::INDEX,
    });
    full.wireframe_index_count = wireframe_index_count;

    has_geometry
}

pub(super) fn install_bvh_and_cull(
    device: &wgpu::Device,
    full: &mut FullRenderer,
    geometry: &LevelGeometry<'_>,
    bvh_leaves: Vec<postretro_render_data::geometry::BvhLeaf>,
    has_multi_draw_indirect: bool,
) {
    // --- BVH + compute cull ---
    full.bvh_leaves = bvh_leaves;
    // Per-cell draw index for the candidate-cull path. Cloned alongside the
    // BVH leaves; the empty-geometry install path clears it to `None`, so
    // `release_level_resources` drops it for free.
    full.cell_draw_index = geometry.cell_draw_index.cloned();
    // Reset per-level so a corrupt index on a later level still warns once.
    full.candidate_cull_oor_logged = false;
    full.compute_cull = if !full.bvh_leaves.is_empty() {
        Some(ComputeCullPipeline::new(
            device,
            geometry.bvh,
            has_multi_draw_indirect,
        ))
    } else {
        None
    };
    // Rebuild the candidate-cull path in lockstep with `compute_cull`, sized
    // to the freshly-installed leaf count. Empty-geometry install → `None`,
    // so `release_level_resources` drops it for free.
    full.candidate_cull = full
        .compute_cull
        .as_ref()
        .map(|c| crate::candidate_cull::CandidateCullPipeline::new(device, c.total_leaves()));
}
