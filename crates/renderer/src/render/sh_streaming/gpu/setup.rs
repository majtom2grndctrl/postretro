//! Initial streamed-pool construction and adapter-limit preflight.
//!
//! Every wgpu allocation is preceded by the capacity checks in the shared
//! helpers. The resulting active pool owns dense textures, fixed indirection,
//! and independently growable compose carriers.

use super::*;
use crate::render::gpu_create::GpuCreate;

impl StreamingGpuPools {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::render::sh_streaming) fn new(
        device: &wgpu::Device,
        limits: &wgpu::Limits,
        queue: &dyn crate::render::queue_writes::QueueWrites,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        sparse_compose: std::sync::Arc<SparseComposeLayouts>,
        initial_pool: InitialPool,
        probe_occlusion_enabled: bool,
        sh: &mut ShVolumeResources,
        pipelines: &super::super::StreamingShPipelines,
        selection_weights: &wgpu::Buffer,
    ) -> Result<Self, ShResidencyDrainError> {
        let shape = AtlasShape::for_slots(
            initial_pool.dense_slots,
            initial_pool.whole_map.dense_slots,
            limits,
        )?;
        preflight_initial_resource_limits(
            base,
            sources,
            &sparse_compose,
            shape,
            &initial_pool.sparse_capacities,
            limits,
        )?;
        Self::new_with_shape(
            device,
            limits,
            queue,
            base,
            sources,
            sparse_compose,
            shape,
            &initial_pool.sparse_capacities,
            initial_pool.whole_map,
            probe_occlusion_enabled,
            sh,
            pipelines,
            selection_weights,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::render::sh_streaming::gpu) fn new_with_shape(
        device: &wgpu::Device,
        limits: &wgpu::Limits,
        queue: &dyn crate::render::queue_writes::QueueWrites,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        sparse_compose: std::sync::Arc<SparseComposeLayouts>,
        shape: AtlasShape,
        sparse_capacities: &SparseCapacities,
        whole_map: WholeMapCap,
        probe_occlusion_enabled: bool,
        sh: &mut ShVolumeResources,
        pipelines: &super::super::StreamingShPipelines,
        selection_weights: &wgpu::Buffer,
    ) -> Result<Self, ShResidencyDrainError> {
        let extent = shape.extent();
        let dense = DenseTextures::new(device, queue, base, sources, shape, sh)?;
        let direct_compose_required = dense.direct_atlas.is_some();

        let depth_extent = wgpu::Extent3d {
            width: base.grid_dimensions[0].max(1),
            height: base.grid_dimensions[1].max(1),
            depth_or_array_layers: base.grid_dimensions[2].max(1),
        };
        if depth_extent.width > limits.max_texture_dimension_3d
            || depth_extent.height > limits.max_texture_dimension_3d
            || depth_extent.depth_or_array_layers > limits.max_texture_dimension_3d
        {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed SH depth-moment grid exceeds adapter 3D texture limits",
            });
        }
        let depth_moments = device.new_texture(&wgpu::TextureDescriptor {
            label: Some("Streamed SH Depth Moments"),
            size: depth_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba16Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let indirection_len = u64::try_from(base.probes.len())
            .ok()
            .and_then(|count| count.checked_mul(u64::from(std::mem::size_of::<u32>() as u32)))
            .ok_or(ShResidencyDrainError::SlotOverflow)?
            .max(4);
        let compose_indirection = device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("Streamed SH Compose Indirection"),
            size: indirection_len,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let sampled_indirection = device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("Streamed SH Sampled Indirection Mirror"),
            size: indirection_len,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let grid_info = create_grid_info(
            device,
            base,
            shape,
            probe_occlusion_enabled,
            sh.depth_tiles.grid_words(),
        );
        let indirect_compose = StreamingIndirectCompose::new(
            device,
            limits,
            base,
            sources,
            shape,
            &dense.base_view,
            &dense.total_storage_view,
            &compose_indirection,
            sh,
            pipelines,
            &sparse_compose.indirect,
            sparse_capacities.get(&27).copied(),
        )?;
        let direct_compose = dense
            .direct_views(selection_weights)?
            .map(|views| {
                StreamingDirectCompose::new(
                    device,
                    limits,
                    base,
                    sources,
                    shape,
                    views,
                    &compose_indirection,
                    sh,
                    pipelines,
                    &sparse_compose,
                    sparse_capacities,
                )
            })
            .transpose()?;
        let (bind_group, mesh_bind_group) =
            create_sample_bind_groups(device, sh, &dense, &grid_info, &depth_moments);
        if direct_compose_required {
            log_streamed_direct_pool_bytes(extent, dense.direct_intermediate.is_some());
        }

        let active_capacity_bytes = texture_bytes(dense.base_format, extent)
            .checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Float, extent))
            .and_then(|bytes| {
                dense.direct_format.map_or(Some(bytes), |format| {
                    bytes.checked_add(texture_bytes(format, extent))
                })
            })
            .and_then(|bytes| {
                dense.direct_composed_pools().try_fold(bytes, |bytes, _| {
                    bytes.checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Float, extent))
                })
            })
            .and_then(|bytes| bytes.checked_add(indirect_compose.active_pool_bytes()))
            .and_then(|bytes| {
                bytes.checked_add(
                    direct_compose
                        .as_ref()
                        .map_or(0, StreamingDirectCompose::active_capacity_bytes),
                )
            })
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let fixed_metadata_bytes = indirection_len
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(96))
            .and_then(|bytes| {
                bytes.checked_add(texture_bytes(
                    wgpu::TextureFormat::Rgba16Uint,
                    wgpu::Extent3d {
                        width: base.grid_dimensions[0].max(1),
                        height: base.grid_dimensions[1].max(1),
                        depth_or_array_layers: base.grid_dimensions[2].max(1),
                    },
                ))
            })
            .and_then(|bytes| bytes.checked_add(indirect_compose.fixed_metadata_bytes()))
            .and_then(|bytes| {
                bytes.checked_add(
                    direct_compose
                        .as_ref()
                        .map_or(0, StreamingDirectCompose::fixed_metadata_bytes),
                )
            })
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        // Animation/sample/scripted descriptor buffers are shared immutable
        // ShVolumeResources allocations, already represented by static ledger
        // rows, so the live streaming summary must not charge them again.

        let dense_capacity_bytes = active_capacity_bytes
            .checked_sub(indirect_compose.active_pool_bytes())
            .and_then(|bytes| {
                bytes.checked_sub(
                    direct_compose
                        .as_ref()
                        .map_or(0, StreamingDirectCompose::active_capacity_bytes),
                )
            })
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        log::info!(
            "[Renderer] streamed SH initial dense pool: bytes={dense_capacity_bytes} tiles_per_layer={} layers={} slots={}",
            shape.tiles_per_layer,
            shape.layers,
            shape.slots
        );
        Ok(Self {
            sparse_compose,
            pipelines: pipelines.clone(),
            shape,
            limits: limits.clone(),
            base_format: dense.base_format,
            direct_format: dense.direct_format,
            base: dense.base,
            total: dense.total,
            base_view: dense.base_view,
            total_storage_view: dense.total_storage_view,
            total_sampled_view: dense.total_sampled_view,
            direct_base: dense.direct_base,
            direct_base_view: dense.direct_base_view,
            direct_atlas: dense.direct_atlas,
            direct_handoff: dense.direct_handoff,
            direct_intermediate: dense.direct_intermediate,
            depth_moments,
            compose_indirection,
            sampled_indirection,
            grid_info,
            indirect_compose,
            direct_compose,
            has_animated_direct_pass: sources.animated_direct_delta.is_some(),
            bind_group,
            mesh_bind_group,
            retiring_dense: None,
            retiring_indirect: None,
            retiring_direct_promotion: None,
            retiring_direct_animated: None,
            active_capacity_bytes,
            fixed_metadata_bytes,
            whole_resident_scatter_bytes: sh.billboard_direct_scatter.capacity_bytes(),
            whole_map,
            probe_occlusion_enabled,
            sparse_capacities: sparse_capacities.clone(),
            growth: PoolGrowthCounters::default(),
            staging: StagingPool::default(),
            upload_scratch: Vec::new(),
        })
    }
}

/// Logs the streamed direct pool pair once per load, at its initial size: a
/// growth reallocates every dense pool at the grown extent.
fn log_streamed_direct_pool_bytes(extent: wgpu::Extent3d, has_intermediate: bool) {
    let pool_bytes = texture_bytes(wgpu::TextureFormat::Rgba16Float, extent);
    let intermediate_bytes = if has_intermediate { pool_bytes } else { 0 };
    log::info!(
        "[Renderer] streamed direct SH pools: direct (promotion-free) {pool_bytes} B, promotion handoff {pool_bytes} B, compose intermediate {intermediate_bytes} B ({}x{}x{} Rgba16Float)",
        extent.width,
        extent.height,
        extent.depth_or_array_layers,
    );
}
