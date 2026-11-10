//! Pipeline carriers for the streamed direct-SH promotion and animated passes.

use crate::render::gpu_create::GpuCreate;
use postretro_level_loader::ShStreamBaseMetadata;
use postretro_render_cpu::frame_uniforms::LightTermMask;
use postretro_render_cpu::sh_compose::ComposeGridParams;
use postretro_render_cpu::sh_volume::STREAMED_SH_PHYSICAL_TILE_STRIDE;

use super::super::gpu::StagedUploads;
use super::super::{ShResidencyDrainError, SparseComposeLayout};
use super::layout::{
    ANIMATED_LIGHT_SCALE_SIZE, DEBUG_OVERRIDE_SIZE, debug_override_bytes, dynamic_grid_entry,
    sampler_entry, storage_entry, storage_texture_entry, texture_entry, uniform_entry,
};
use super::sparse::{
    DirectSparseRowUpload, RetiredDirectSparseResources, StreamingSparseBuffers,
    build_grid_and_sparse, checked_ledger_sum,
};
use super::{AnimatedViews, PromotionOutputs};
use crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
use crate::render::animated_direct_sh_compose::BIND_ANIMATED_DIRECT_BASE;
use crate::render::direct_sh_compose::{
    BIND_AFFINITY_LIGHTS, BIND_AFFINITY_OFFSETS, BIND_ANIMATION_DESCRIPTORS,
    BIND_ANIMATION_SAMPLES, BIND_BASE_SAMPLER, BIND_DELTA_COMPACTION_META, BIND_DELTA_SUBBLOCKS,
    BIND_DIRECT_ATLAS_OUTPUT, BIND_PROBE_INDIRECTION, DirectShDebugOverride,
    direct_compose_params_bytes, nearest_sampler,
};
use crate::render::mover_channel::BIND_MOVER_CHANNEL;
use crate::render::renderer_types::PromotedBakedLightState;
use crate::render::sh_compose_dispatch::{
    DynamicComposeGridUpload, build_dynamic_compose_grid_upload_for_rows_into,
};
use crate::render::sh_streaming::StreamingShPipelines;
use crate::render::sh_streaming::gpu::AtlasShape;
use crate::render::sh_volume::ShVolumeResources;

const BIND_SELECTION_WEIGHTS: u32 = 26;
const BIND_DEBUG_OVERRIDE: u32 = 27;
const BIND_FRAME_LIGHT_TERM_MASK: u32 = 29;
const BIND_ANIMATED_LIGHT_SCALE: u32 = 26;
const BIND_ANIMATED_COMPACTION_META: u32 = 27;
const BIND_ANIMATED_PROBE_INDIRECTION: u32 = 28;
pub(in crate::render::sh_streaming) const ANIMATED_DIRECT_COMPOSE_ENTRY_POINT: &str =
    "animated_compose_main";

pub(super) struct StreamingPromotionPass {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    sparse: StreamingSparseBuffers,
    grid: ComposeGridParams,
    grid_buffer: wgpu::Buffer,
    grid_capacity: u64,
    fixed_metadata_bytes: u64,
    active_capacity_bytes: u64,
    max_workgroups_x: u32,
    dynamic_alignment: u32,
    max_buffer_size: u64,
    light_term_mask: wgpu::Buffer,
    last_light_term_mask: LightTermMask,
    /// Fixed for the pass's life: no id 45 means Pass A writes the
    /// promotion-free atlas itself.
    stores_promotion_free: bool,
    debug_override: wgpu::Buffer,
    last_debug_override: [u8; DEBUG_OVERRIDE_SIZE],
    grid_upload: DynamicComposeGridUpload,
    /// Id 57's table and the frame's mover t′ (`MoverChannelResources`).
    mover_channel: wgpu::Buffer,
}

impl StreamingPromotionPass {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        device: &wgpu::Device,
        limits: &wgpu::Limits,
        base: &ShStreamBaseMetadata,
        layout: &SparseComposeLayout,
        shape: AtlasShape,
        base_view: &wgpu::TextureView,
        outputs: PromotionOutputs<'_>,
        selection_weights: &wgpu::Buffer,
        compose_indirection: &wgpu::Buffer,
        mover_channel: &wgpu::Buffer,
        pipelines: &StreamingShPipelines,
        sparse_capacity: Option<(u32, u32)>,
    ) -> Result<Self, ShResidencyDrainError> {
        let (grid, sparse, grid_buffer, grid_capacity) = build_grid_and_sparse(
            device,
            limits,
            base,
            layout,
            shape,
            "Streamed Direct SH Promotion Grid Records",
            sparse_capacity,
        )?;
        let sampler = nearest_sampler(device, "Streamed Direct SH Base Sampler");
        let initial_debug_override = debug_override_bytes(DirectShDebugOverride::default());
        let debug_override = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Streamed Direct SH Debug Override"),
            contents: &initial_debug_override,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let light_term_mask = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Streamed Direct SH Light-Term Mask"),
            contents: &direct_compose_params_bytes(
                LightTermMask::ALL,
                outputs.stores_promotion_free,
            ),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bgl = pipelines.promotion.bind_group_layout.clone();
        let pipeline = pipelines.promotion.pipeline.clone();
        let stores_promotion_free = outputs.stores_promotion_free;
        let bind_group = Self::build_bind_group(
            device,
            &bgl,
            base_view,
            &outputs,
            &sampler,
            &grid_buffer,
            &sparse,
            selection_weights,
            &debug_override,
            &light_term_mask,
            compose_indirection,
            mover_channel,
        );
        let fixed_metadata_bytes = checked_ledger_sum(&[
            grid_capacity,
            sparse.fixed_metadata_bytes(),
            light_term_mask.size(),
            debug_override.size(),
        ])?;
        let active_capacity_bytes = sparse.active_capacity_bytes();
        Ok(Self {
            pipeline,
            bind_group_layout: bgl,
            bind_group,
            sampler,
            sparse,
            grid,
            grid_buffer,
            grid_capacity,
            fixed_metadata_bytes,
            active_capacity_bytes,
            max_workgroups_x: limits.max_compute_workgroups_per_dimension,
            dynamic_alignment: limits.min_uniform_buffer_offset_alignment,
            max_buffer_size: limits.max_buffer_size,
            light_term_mask,
            last_light_term_mask: LightTermMask::ALL,
            stores_promotion_free,
            debug_override,
            last_debug_override: initial_debug_override,
            grid_upload: DynamicComposeGridUpload::default(),
            mover_channel: mover_channel.clone(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        base_view: &wgpu::TextureView,
        outputs: &PromotionOutputs<'_>,
        sampler: &wgpu::Sampler,
        grid_buffer: &wgpu::Buffer,
        sparse: &StreamingSparseBuffers,
        selection_weights: &wgpu::Buffer,
        debug_override: &wgpu::Buffer,
        light_term_mask: &wgpu::Buffer,
        compose_indirection: &wgpu::Buffer,
        mover_channel: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.new_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Streamed Direct SH Promotion Bind Group"),
            layout,
            entries: &[
                texture_entry(0, base_view),
                storage_texture_entry(1, outputs.handoff),
                storage_texture_entry(BIND_DIRECT_ATLAS_OUTPUT, outputs.promotion_free),
                sampler_entry(BIND_BASE_SAMPLER, sampler),
                dynamic_grid_entry(grid_buffer),
                storage_entry(BIND_DELTA_SUBBLOCKS, sparse.tile_words()),
                storage_entry(BIND_AFFINITY_OFFSETS, sparse.row_pairs()),
                storage_entry(BIND_AFFINITY_LIGHTS, sparse.lights()),
                storage_entry(BIND_SELECTION_WEIGHTS, selection_weights),
                uniform_entry(BIND_DEBUG_OVERRIDE, debug_override),
                storage_entry(BIND_DELTA_COMPACTION_META, sparse.compaction_metadata()),
                uniform_entry(BIND_FRAME_LIGHT_TERM_MASK, light_term_mask),
                storage_entry(BIND_PROBE_INDIRECTION, compose_indirection),
                storage_entry(BIND_MOVER_CHANNEL, mover_channel),
            ],
        })
    }

    /// Rewire only the dense atlas side of this pass, and the mover
    /// channel's live table. The CSR backing and dynamic-record buffers
    /// remain in their id-41 pool generation.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn rebind_dense(
        &mut self,
        device: &wgpu::Device,
        shape: AtlasShape,
        base_view: &wgpu::TextureView,
        outputs: PromotionOutputs<'_>,
        selection_weights: &wgpu::Buffer,
        compose_indirection: &wgpu::Buffer,
        mover_channel: &wgpu::Buffer,
    ) {
        self.mover_channel = mover_channel.clone();
        debug_assert_eq!(outputs.stores_promotion_free, self.stores_promotion_free);
        self.grid.atlas_dimensions = [shape.extent().width, shape.extent().height];
        self.grid.atlas_tiles_per_row = shape.tiles_per_row;
        self.grid.tiles_per_layer = shape.tiles_per_layer;
        self.grid.atlas_layer_count = shape.layers;
        self.grid.compact_atlas_tiles_per_row = shape.tiles_per_row;
        self.grid.compact_atlas_tiles_per_layer = shape.tiles_per_layer;
        let replacement = Self::build_bind_group(
            device,
            &self.bind_group_layout,
            base_view,
            &outputs,
            &self.sampler,
            &self.grid_buffer,
            &self.sparse,
            selection_weights,
            &self.debug_override,
            &self.light_term_mask,
            compose_indirection,
            &self.mover_channel,
        );
        self.bind_group = replacement;
    }

    pub(super) fn upload_sparse_rows(
        &self,
        uploads: &mut StagedUploads,
        rows: &[DirectSparseRowUpload<'_>],
    ) -> Result<(), ShResidencyDrainError> {
        self.sparse.upload_rows(uploads, rows)
    }

    pub(super) fn validate_sparse_rows(
        &self,
        rows: &[DirectSparseRowUpload<'_>],
        capacity: (u32, u32),
    ) -> Result<(), ShResidencyDrainError> {
        self.sparse.validate_rows(rows, capacity)
    }

    pub(super) fn clear_row_pair(
        &self,
        uploads: &mut StagedUploads,
        row: u32,
    ) -> Result<(), ShResidencyDrainError> {
        self.sparse.clear_row_pair(uploads, row)
    }

    pub(super) fn clear_all_row_pairs(
        &self,
        queue: &crate::render::uploads::UploadQueue,
    ) -> Result<(), ShResidencyDrainError> {
        self.sparse.clear_all_row_pairs(queue)
    }

    pub(super) fn dispatch(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        encoder: &mut wgpu::CommandEncoder,
        light_term_mask: LightTermMask,
        debug_override: DirectShDebugOverride,
        rows: &[u32],
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) -> Result<usize, ShResidencyDrainError> {
        if light_term_mask != self.last_light_term_mask {
            queue.write_buffer(
                &self.light_term_mask,
                0,
                &direct_compose_params_bytes(light_term_mask, self.stores_promotion_free),
            );
            self.last_light_term_mask = light_term_mask;
        }
        let debug_bytes = debug_override_bytes(debug_override);
        if debug_bytes != self.last_debug_override {
            queue.write_buffer(&self.debug_override, 0, &debug_bytes);
            self.last_debug_override = debug_bytes;
        }
        dispatch_dynamic_pass(
            queue,
            encoder,
            "Streamed Direct SH Promotion",
            &self.pipeline,
            &self.bind_group,
            None,
            self.grid,
            &self.grid_buffer,
            self.grid_capacity,
            self.max_workgroups_x,
            self.dynamic_alignment,
            self.max_buffer_size,
            rows,
            &mut self.grid_upload,
            timestamp_writes,
        )
    }

    pub(super) fn fixed_metadata_bytes(&self) -> u64 {
        self.fixed_metadata_bytes
    }

    pub(super) fn active_capacity_bytes(&self) -> u64 {
        self.active_capacity_bytes
    }

    pub(super) const fn entry_capacity(&self) -> u32 {
        self.sparse.entry_capacity()
    }

    pub(super) const fn tile_f16_capacity(&self) -> u32 {
        self.sparse.tile_f16_capacity()
    }

    pub(super) fn retired_sparse_capacity_bytes(&self) -> Result<u64, ShResidencyDrainError> {
        self.sparse.retired_sparse_capacity_bytes()
    }

    pub(super) fn into_retired_sparse_resources(self) -> RetiredDirectSparseResources {
        self.sparse.into_retired_sparse_resources()
    }

    pub(super) fn copy_retained_to(&self, destination: &Self, encoder: &mut wgpu::CommandEncoder) {
        self.sparse.copy_retained_to(&destination.sparse, encoder);
    }
}

pub(super) struct StreamingAnimatedPass {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    sparse: StreamingSparseBuffers,
    grid: ComposeGridParams,
    grid_buffer: wgpu::Buffer,
    grid_capacity: u64,
    fixed_metadata_bytes: u64,
    active_capacity_bytes: u64,
    max_workgroups_x: u32,
    dynamic_alignment: u32,
    max_buffer_size: u64,
    /// Id 57's table, which also carries this pass's descriptor map.
    mover_channel: wgpu::Buffer,
    light_scale: wgpu::Buffer,
    last_light_scale: [u8; ANIMATED_LIGHT_SCALE_SIZE],
    grid_upload: DynamicComposeGridUpload,
}

impl StreamingAnimatedPass {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        device: &wgpu::Device,
        limits: &wgpu::Limits,
        base: &ShStreamBaseMetadata,
        shape: AtlasShape,
        views: AnimatedViews<'_>,
        compose_indirection: &wgpu::Buffer,
        sh: &ShVolumeResources,
        pipelines: &StreamingShPipelines,
        layout: &SparseComposeLayout,
        sparse_capacity: Option<(u32, u32)>,
    ) -> Result<Self, ShResidencyDrainError> {
        let (grid, sparse, grid_buffer, grid_capacity) = build_grid_and_sparse(
            device,
            limits,
            base,
            layout,
            shape,
            "Streamed Animated Direct SH Grid Records",
            sparse_capacity,
        )?;
        let mover_channel = sh.mover_channel.buffer().clone();
        // Default scale bytes are all compose weights == 1.0. This prevents a
        // first-frame id-45 compose from silently erasing every animation.
        let initial_light_scale = AnimatedDirectShDebugOverride::default().bytes(&[]);
        let light_scale = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Streamed Animated Direct SH Light Scale"),
            contents: &initial_light_scale,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sampler = nearest_sampler(device, "Streamed Animated Direct SH Sampler");
        let bgl = pipelines.animated.bind_group_layout.clone();
        let pipeline = pipelines.animated.pipeline.clone();
        let bind_group = Self::build_bind_group(
            device,
            &bgl,
            &views,
            &sampler,
            &grid_buffer,
            &sparse,
            &sh.animation.descriptors,
            &sh.animation.anim_samples,
            &mover_channel,
            &light_scale,
            compose_indirection,
        );
        let fixed_metadata_bytes = checked_ledger_sum(&[
            grid_capacity,
            sparse.fixed_metadata_bytes(),
            light_scale.size(),
        ])?;
        let active_capacity_bytes = sparse.active_capacity_bytes();
        Ok(Self {
            pipeline,
            bind_group_layout: bgl,
            bind_group,
            sampler,
            sparse,
            grid,
            grid_buffer,
            grid_capacity,
            fixed_metadata_bytes,
            active_capacity_bytes,
            max_workgroups_x: limits.max_compute_workgroups_per_dimension,
            dynamic_alignment: limits.min_uniform_buffer_offset_alignment,
            max_buffer_size: limits.max_buffer_size,
            mover_channel,
            light_scale,
            last_light_scale: initial_light_scale,
            grid_upload: DynamicComposeGridUpload::default(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        views: &AnimatedViews<'_>,
        sampler: &wgpu::Sampler,
        grid_buffer: &wgpu::Buffer,
        sparse: &StreamingSparseBuffers,
        animation_descriptors: &wgpu::Buffer,
        animation_samples: &wgpu::Buffer,
        mover_channel: &wgpu::Buffer,
        light_scale: &wgpu::Buffer,
        compose_indirection: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.new_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Streamed Animated Direct SH Bind Group"),
            layout,
            entries: &[
                texture_entry(0, views.intermediate_sampled),
                storage_texture_entry(1, views.handoff_storage),
                texture_entry(BIND_ANIMATED_DIRECT_BASE, views.base_sampled),
                storage_texture_entry(BIND_DIRECT_ATLAS_OUTPUT, views.promotion_free_storage),
                sampler_entry(BIND_BASE_SAMPLER, sampler),
                dynamic_grid_entry(grid_buffer),
                storage_entry(BIND_DELTA_SUBBLOCKS, sparse.tile_words()),
                storage_entry(BIND_AFFINITY_OFFSETS, sparse.row_pairs()),
                storage_entry(BIND_ANIMATION_DESCRIPTORS, animation_descriptors),
                storage_entry(BIND_ANIMATION_SAMPLES, animation_samples),
                storage_entry(BIND_AFFINITY_LIGHTS, sparse.lights()),
                storage_entry(BIND_MOVER_CHANNEL, mover_channel),
                uniform_entry(BIND_ANIMATED_LIGHT_SCALE, light_scale),
                storage_entry(BIND_ANIMATED_COMPACTION_META, sparse.compaction_metadata()),
                storage_entry(BIND_ANIMATED_PROBE_INDIRECTION, compose_indirection),
            ],
        })
    }
}

mod animated_runtime;

#[allow(clippy::too_many_arguments)]
fn dispatch_dynamic_pass(
    queue: &crate::render::uploads::UploadQueue,
    encoder: &mut wgpu::CommandEncoder,
    label: &'static str,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    uniform_bind_group: Option<&wgpu::BindGroup>,
    grid: ComposeGridParams,
    grid_buffer: &wgpu::Buffer,
    grid_capacity: u64,
    max_workgroups_x: u32,
    dynamic_alignment: u32,
    max_buffer_size: u64,
    rows: &[u32],
    grid_upload: &mut DynamicComposeGridUpload,
    timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
) -> Result<usize, ShResidencyDrainError> {
    build_dynamic_compose_grid_upload_for_rows_into(
        grid_upload,
        grid,
        STREAMED_SH_PHYSICAL_TILE_STRIDE,
        rows,
        max_workgroups_x,
        dynamic_alignment,
        max_buffer_size,
    )
    .ok_or(ShResidencyDrainError::GpuCapacity {
        reason: "streamed direct dirty compose range exceeds adapter limits",
    })?;
    let upload = &*grid_upload;
    if upload.dispatches.is_empty() {
        return Ok(0);
    }
    if u64::try_from(upload.bytes.len()).map_err(|_| ShResidencyDrainError::SlotOverflow)?
        > grid_capacity
    {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed direct dirty compose record count exceeds pool capacity",
        });
    }
    queue.write_buffer(grid_buffer, 0, &upload.bytes);
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some(label),
        timestamp_writes,
    });
    pass.set_pipeline(pipeline);
    if let Some(uniform_bind_group) = uniform_bind_group {
        pass.set_bind_group(0, uniform_bind_group, &[]);
        for dispatch in &upload.dispatches {
            pass.set_bind_group(1, bind_group, &[dispatch.dynamic_offset]);
            pass.dispatch_workgroups(dispatch.workgroup_count, 1, 1);
        }
    } else {
        for dispatch in &upload.dispatches {
            pass.set_bind_group(0, bind_group, &[dispatch.dynamic_offset]);
            pass.dispatch_workgroups(dispatch.workgroup_count, 1, 1);
        }
    }
    Ok(upload.dispatches.len())
}

#[cfg(test)]
mod tests {
    use super::ANIMATED_DIRECT_COMPOSE_ENTRY_POINT;

    // Regression: streamed animated-direct SH startup selected a nonexistent WGSL entry point.
    #[test]
    fn streamed_animated_direct_pipeline_entry_point_exists() {
        let source = [
            include_str!("../../../shaders/animated_direct_sh_compose.wgsl"),
            "\n",
            include_str!("../../../shaders/curve_eval.wgsl"),
            "\n",
            crate::render::mover_channel::MOVER_CHANNEL_WGSL,
            "\n",
            crate::render::sh_indirection::WGSL_DECODE_HELPER,
        ]
        .concat();
        let module = naga::front::wgsl::parse_str(&source)
            .expect("streamed animated direct SH shader should parse");
        assert!(module.entry_points.iter().any(|entry| {
            entry.name == ANIMATED_DIRECT_COMPOSE_ENTRY_POINT
                && entry.stage == naga::ShaderStage::Compute
        }));
    }
}
