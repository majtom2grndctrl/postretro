//! Sample-side bind groups and streamed-grid uniform construction.

use super::*;
use crate::render::direct_sh_resources::{mesh_superset_entries, shared_direct_atlas_entry};
use crate::render::gpu_create::GpuCreate;

pub(super) fn create_sample_bind_groups(
    device: &wgpu::Device,
    sh: &ShVolumeResources,
    dense: &DenseTextures,
    grid_info: &wgpu::Buffer,
    depth_moments: &wgpu::Texture,
) -> (wgpu::BindGroup, wgpu::BindGroup) {
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Streamed SH Atlas Sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let depth_view = depth_moments.create_view(&wgpu::TextureViewDescriptor {
        label: Some("Streamed SH Depth Moment View"),
        dimension: Some(wgpu::TextureViewDimension::D3),
        ..Default::default()
    });
    let (entries, mesh_entries) =
        sample_bind_group_entries(sh, dense, grid_info, &depth_view, &sampler);
    let bind_group = device.new_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Streamed SH Volume Bind Group"),
        layout: &sh.bind_group_layout,
        entries: &entries,
    });
    let mesh_bind_group = device.new_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Streamed SH Mesh Bind Group"),
        layout: &sh.mesh_bind_group_layout,
        entries: &mesh_entries,
    });
    (bind_group, mesh_bind_group)
}

/// The shared group-3 entries and the mesh superset's. Binding 15 differs:
/// the shared group samples the promotion-free direct atlas, the mesh
/// superset the promotion handoff atlas.
pub(super) fn sample_bind_group_entries<'a>(
    sh: &'a ShVolumeResources,
    dense: &'a DenseTextures,
    grid_info: &'a wgpu::Buffer,
    depth_view: &'a wgpu::TextureView,
    sampler: &'a wgpu::Sampler,
) -> (Vec<wgpu::BindGroupEntry<'a>>, Vec<wgpu::BindGroupEntry<'a>>) {
    let (direct_view, handoff_view) = dense.direct_sample_views(sh);
    let entries = vec![
        wgpu::BindGroupEntry {
            binding: BIND_SH_TOTAL_ATLAS,
            resource: wgpu::BindingResource::TextureView(&dense.total_sampled_view),
        },
        wgpu::BindGroupEntry {
            binding: BIND_SH_ATLAS_SAMPLER,
            resource: wgpu::BindingResource::Sampler(sampler),
        },
        wgpu::BindGroupEntry {
            binding: BIND_SH_GRID_INFO,
            resource: grid_info.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: BIND_ANIM_DESCRIPTORS,
            resource: sh.animation.descriptors.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: BIND_ANIM_SAMPLES,
            resource: sh.animation.anim_samples.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: BIND_SCRIPTED_LIGHT_DESCRIPTORS,
            resource: sh.scripted_light_descriptors.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: BIND_SH_DEPTH_MOMENTS,
            resource: wgpu::BindingResource::TextureView(depth_view),
        },
        // Whole-resident, so growth rebinds the level's one tile texture.
        sh.depth_tiles.bind_group_entry(),
        shared_direct_atlas_entry(direct_view),
        wgpu::BindGroupEntry {
            binding: BIND_BILLBOARD_DIRECT_SCATTER,
            resource: wgpu::BindingResource::TextureView(&sh.billboard_direct_scatter.sampled_view),
        },
    ];
    let mesh_entries = mesh_superset_entries(
        &entries,
        handoff_view,
        sh.direct.dynamic_direct_params_binding(),
    );
    (entries, mesh_entries)
}

pub(super) fn create_grid_info(
    device: &wgpu::Device,
    base: &ShStreamBaseMetadata,
    shape: AtlasShape,
    probe_occlusion_enabled: bool,
    depth_tiles: DepthTileGridWords,
) -> wgpu::Buffer {
    let extent = shape.extent();
    let grid_bytes = build_grid_info_bytes(ShGridInfoParams {
        grid_origin: base.grid_origin,
        cell_size: base.cell_size,
        grid_dimensions: base.grid_dimensions,
        atlas_dimensions: [extent.width, extent.height],
        tile_dimension: base.tile_dimension,
        tile_border: base.tile_border,
        atlas_tiles_per_row: shape.tiles_per_row,
        physical_tile_stride: STREAMED_SH_PHYSICAL_TILE_STRIDE,
        tiles_per_layer: shape.tiles_per_layer,
        atlas_layer_count: shape.layers,
        present: true,
        probe_occlusion_enabled,
        depth_tiles,
    });
    device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Streamed SH Grid Info"),
        contents: &grid_bytes,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}
