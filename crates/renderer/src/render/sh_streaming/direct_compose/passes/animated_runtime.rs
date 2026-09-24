use super::*;

impl StreamingAnimatedPass {
    /// Rewire the two dense texture views without reallocating or copying the
    /// independent id-45 sparse backing generation.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::render::sh_streaming::direct_compose) fn rebind_dense(
        &mut self,
        device: &wgpu::Device,
        shape: AtlasShape,
        intermediate_sampled: &wgpu::TextureView,
        output_storage: &wgpu::TextureView,
        compose_indirection: &wgpu::Buffer,
        sh: &ShVolumeResources,
    ) {
        self.grid.atlas_dimensions = [shape.extent().width, shape.extent().height];
        self.grid.atlas_tiles_per_row = shape.tiles_per_row;
        self.grid.tiles_per_layer = shape.tiles_per_layer;
        self.grid.atlas_layer_count = shape.layers;
        self.grid.compact_atlas_tiles_per_row = shape.tiles_per_row;
        self.grid.compact_atlas_tiles_per_layer = shape.tiles_per_layer;
        let replacement = Self::build_bind_group(
            device,
            &self.bind_group_layout,
            intermediate_sampled,
            output_storage,
            &self.sampler,
            &self.grid_buffer,
            &self.sparse,
            &sh.animation.descriptors,
            &sh.animation.anim_samples,
            &self.descriptor_indices,
            &self.light_scale,
            compose_indirection,
        );
        self.bind_group = replacement;
    }

    pub(in crate::render::sh_streaming::direct_compose) fn upload_sparse_rows(
        &self,
        uploads: &mut StagedUploads,
        rows: &[DirectSparseRowUpload<'_>],
    ) -> Result<(), ShResidencyDrainError> {
        self.sparse.upload_rows(uploads, rows)
    }

    pub(in crate::render::sh_streaming::direct_compose) fn validate_sparse_rows(
        &self,
        rows: &[DirectSparseRowUpload<'_>],
    ) -> Result<(), ShResidencyDrainError> {
        self.sparse.validate_rows(rows)
    }

    pub(in crate::render::sh_streaming::direct_compose) fn clear_row_pair(
        &self,
        uploads: &mut StagedUploads,
        row: u32,
    ) -> Result<(), ShResidencyDrainError> {
        self.sparse.clear_row_pair(uploads, row)
    }

    pub(in crate::render::sh_streaming::direct_compose) fn clear_all_row_pairs(
        &self,
        queue: &wgpu::Queue,
    ) -> Result<(), ShResidencyDrainError> {
        self.sparse.clear_all_row_pairs(queue)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "GPU dispatch keeps queue, encoder, bindings, dirty ranges, and timestamp writes explicit."
    )]
    pub(in crate::render::sh_streaming::direct_compose) fn dispatch(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        debug_override: AnimatedDirectShDebugOverride,
        promoted_animated_states: &[PromotedBakedLightState],
        dirty_ranges: &[(u32, u32)],
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) -> Result<(), ShResidencyDrainError> {
        let light_scale = debug_override.bytes(promoted_animated_states);
        if light_scale != self.last_light_scale {
            queue.write_buffer(&self.light_scale, 0, &light_scale);
            self.last_light_scale = light_scale;
        }
        dispatch_dynamic_pass(
            queue,
            encoder,
            "Streamed Animated Direct SH",
            &self.pipeline,
            &self.bind_group,
            Some(uniform_bind_group),
            self.grid,
            &self.grid_buffer,
            self.grid_capacity,
            self.max_workgroups_x,
            self.dynamic_alignment,
            self.max_buffer_size,
            dirty_ranges,
            timestamp_writes,
        )
    }

    pub(in crate::render::sh_streaming::direct_compose) fn fixed_metadata_bytes(&self) -> u64 {
        self.fixed_metadata_bytes
    }

    pub(in crate::render::sh_streaming::direct_compose) fn active_capacity_bytes(&self) -> u64 {
        self.active_capacity_bytes
    }

    pub(in crate::render::sh_streaming::direct_compose) const fn entry_capacity(&self) -> u32 {
        self.sparse.entry_capacity()
    }

    pub(in crate::render::sh_streaming::direct_compose) const fn tile_f16_capacity(&self) -> u32 {
        self.sparse.tile_f16_capacity()
    }

    pub(in crate::render::sh_streaming::direct_compose) fn retired_sparse_capacity_bytes(
        &self,
    ) -> Result<u64, ShResidencyDrainError> {
        self.sparse.retired_sparse_capacity_bytes()
    }

    pub(in crate::render::sh_streaming::direct_compose) fn into_retired_sparse_resources(
        self,
    ) -> RetiredDirectSparseResources {
        self.sparse.into_retired_sparse_resources()
    }

    pub(in crate::render::sh_streaming::direct_compose) fn copy_retained_to(
        &self,
        destination: &Self,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        self.sparse.copy_retained_to(&destination.sparse, encoder);
    }
}
