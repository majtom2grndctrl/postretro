//! Pipeline carriers for the streamed direct-SH promotion and animated passes.

use postretro_level_loader::{ShStreamBaseMetadata, ShStreamSparseMetadata};
use postretro_render_cpu::frame_uniforms::LightTermMask;
use postretro_render_cpu::sh_compose::ComposeGridParams;
use postretro_render_cpu::sh_volume::STREAMED_SH_PHYSICAL_TILE_STRIDE;
use wgpu::util::DeviceExt;

use super::super::ShResidencyDrainError;
use super::super::gpu::StagedUploads;
use super::layout::{
    ANIMATED_LIGHT_SCALE_SIZE, DEBUG_OVERRIDE_SIZE, animated_bgl_entries, debug_override_bytes,
    dynamic_grid_entry, light_term_mask_bytes, promotion_bgl_entries, sampler_entry, storage_entry,
    storage_texture_entry, texture_entry, u32_bytes, uniform_entry,
};
use super::sparse::{
    DirectSparseRowUpload, RetiredDirectSparseResources, StreamingSparseBuffers,
    build_grid_and_sparse, checked_ledger_sum,
};
use crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
use crate::render::direct_sh_compose::{
    BIND_AFFINITY_LIGHTS, BIND_AFFINITY_OFFSETS, BIND_ANIMATION_DESCRIPTOR_INDICES,
    BIND_ANIMATION_DESCRIPTORS, BIND_ANIMATION_SAMPLES, BIND_BASE_SAMPLER,
    BIND_DELTA_COMPACTION_META, BIND_DELTA_SUBBLOCKS, BIND_PROBE_INDIRECTION,
    DirectShDebugOverride, nearest_sampler,
};
use crate::render::renderer_types::PromotedBakedLightState;
use crate::render::sh_compose_dispatch::build_dynamic_compose_grid_upload_for_rows;
use crate::render::sh_indirection::WGSL_DECODE_HELPER;
use crate::render::sh_streaming::gpu::AtlasShape;
use crate::render::sh_volume::ShVolumeResources;

const BIND_SELECTION_WEIGHTS: u32 = 26;
const BIND_DEBUG_OVERRIDE: u32 = 27;
const BIND_FRAME_LIGHT_TERM_MASK: u32 = 29;
const BIND_ANIMATED_LIGHT_SCALE: u32 = 26;
const BIND_ANIMATED_COMPACTION_META: u32 = 27;
const BIND_ANIMATED_PROBE_INDIRECTION: u32 = 28;
const ANIMATED_DIRECT_COMPOSE_ENTRY_POINT: &str = "animated_compose_main";

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
    debug_override: wgpu::Buffer,
    last_debug_override: [u8; DEBUG_OVERRIDE_SIZE],
}

impl StreamingPromotionPass {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        device: &wgpu::Device,
        base: &ShStreamBaseMetadata,
        source: Option<&ShStreamSparseMetadata>,
        shape: AtlasShape,
        base_view: &wgpu::TextureView,
        output_storage: &wgpu::TextureView,
        selection_weights: &wgpu::Buffer,
        compose_indirection: &wgpu::Buffer,
        sparse_floor: Option<(u32, u32)>,
    ) -> Result<Self, ShResidencyDrainError> {
        let (grid, sparse, grid_buffer, grid_capacity) = build_grid_and_sparse(
            device,
            base,
            source,
            shape,
            "Streamed Direct SH Promotion Grid Records",
            sparse_floor,
        )?;
        let sampler = nearest_sampler(device, "Streamed Direct SH Base Sampler");
        let initial_debug_override = debug_override_bytes(DirectShDebugOverride::default());
        let debug_override = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Streamed Direct SH Debug Override"),
            contents: &initial_debug_override,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let light_term_mask = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Streamed Direct SH Light-Term Mask"),
            contents: &light_term_mask_bytes(LightTermMask::ALL),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Streamed Direct SH Promotion BGL"),
            entries: &promotion_bgl_entries(),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Streamed Direct SH Promotion Pipeline Layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Streamed Direct SH Promotion Shader"),
            source: wgpu::ShaderSource::Wgsl(
                [
                    include_str!("../../../shaders/direct_sh_compose.wgsl"),
                    "\n",
                    WGSL_DECODE_HELPER,
                ]
                .concat()
                .into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Streamed Direct SH Promotion Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("compose_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let bind_group = Self::build_bind_group(
            device,
            &bgl,
            base_view,
            output_storage,
            &sampler,
            &grid_buffer,
            &sparse,
            selection_weights,
            &debug_override,
            &light_term_mask,
            compose_indirection,
        );
        let fixed_metadata_bytes = checked_ledger_sum(&[
            grid_capacity,
            sparse.fixed_metadata_bytes(),
            light_term_mask.size(),
            debug_override.size(),
        ])?;
        let active_capacity_bytes = sparse.active_capacity_bytes();
        let limits = device.limits();
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
            debug_override,
            last_debug_override: initial_debug_override,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        base_view: &wgpu::TextureView,
        output_storage: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        grid_buffer: &wgpu::Buffer,
        sparse: &StreamingSparseBuffers,
        selection_weights: &wgpu::Buffer,
        debug_override: &wgpu::Buffer,
        light_term_mask: &wgpu::Buffer,
        compose_indirection: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Streamed Direct SH Promotion Bind Group"),
            layout,
            entries: &[
                texture_entry(0, base_view),
                storage_texture_entry(1, output_storage),
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
            ],
        })
    }

    /// Rewire only the dense atlas side of this pass. The CSR backing and
    /// dynamic-record buffers remain in their id-41 pool generation.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn rebind_dense(
        &mut self,
        device: &wgpu::Device,
        shape: AtlasShape,
        base_view: &wgpu::TextureView,
        output_storage: &wgpu::TextureView,
        selection_weights: &wgpu::Buffer,
        compose_indirection: &wgpu::Buffer,
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
            base_view,
            output_storage,
            &self.sampler,
            &self.grid_buffer,
            &self.sparse,
            selection_weights,
            &self.debug_override,
            &self.light_term_mask,
            compose_indirection,
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
    ) -> Result<(), ShResidencyDrainError> {
        self.sparse.validate_rows(rows)
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
        queue: &wgpu::Queue,
    ) -> Result<(), ShResidencyDrainError> {
        self.sparse.clear_all_row_pairs(queue)
    }

    pub(super) fn dispatch(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        light_term_mask: LightTermMask,
        debug_override: DirectShDebugOverride,
        rows: &[u32],
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) -> Result<(), ShResidencyDrainError> {
        if light_term_mask != self.last_light_term_mask {
            queue.write_buffer(
                &self.light_term_mask,
                0,
                &light_term_mask_bytes(light_term_mask),
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
    descriptor_indices: wgpu::Buffer,
    light_scale: wgpu::Buffer,
    last_light_scale: [u8; ANIMATED_LIGHT_SCALE_SIZE],
}

impl StreamingAnimatedPass {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        device: &wgpu::Device,
        base: &ShStreamBaseMetadata,
        source: &ShStreamSparseMetadata,
        shape: AtlasShape,
        intermediate_sampled: &wgpu::TextureView,
        output_storage: &wgpu::TextureView,
        compose_indirection: &wgpu::Buffer,
        sh: &ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        sparse_floor: Option<(u32, u32)>,
    ) -> Result<Self, ShResidencyDrainError> {
        let (grid, sparse, grid_buffer, grid_capacity) = build_grid_and_sparse(
            device,
            base,
            Some(source),
            shape,
            "Streamed Animated Direct SH Grid Records",
            sparse_floor,
        )?;
        let descriptor_indices = if source.animation_descriptor_indices.is_empty() {
            vec![u32::MAX]
        } else {
            source.animation_descriptor_indices.clone()
        };
        let descriptor_indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Streamed Animated Direct SH Descriptor Indices"),
            contents: &u32_bytes(&descriptor_indices),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        // Default scale bytes are all compose weights == 1.0. This prevents a
        // first-frame id-45 compose from silently erasing every animation.
        let initial_light_scale = AnimatedDirectShDebugOverride::default().bytes(&[]);
        let light_scale = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Streamed Animated Direct SH Light Scale"),
            contents: &initial_light_scale,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sampler = nearest_sampler(device, "Streamed Animated Direct SH Sampler");
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Streamed Animated Direct SH BGL"),
            entries: &animated_bgl_entries(),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Streamed Animated Direct SH Pipeline Layout"),
            bind_group_layouts: &[Some(uniform_bind_group_layout), Some(&bgl)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Streamed Animated Direct SH Shader"),
            source: wgpu::ShaderSource::Wgsl(
                [
                    include_str!("../../../shaders/animated_direct_sh_compose.wgsl"),
                    "\n",
                    include_str!("../../../shaders/curve_eval.wgsl"),
                    "\n",
                    WGSL_DECODE_HELPER,
                ]
                .concat()
                .into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Streamed Animated Direct SH Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some(ANIMATED_DIRECT_COMPOSE_ENTRY_POINT),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let bind_group = Self::build_bind_group(
            device,
            &bgl,
            intermediate_sampled,
            output_storage,
            &sampler,
            &grid_buffer,
            &sparse,
            &sh.animation.descriptors,
            &sh.animation.anim_samples,
            &descriptor_indices,
            &light_scale,
            compose_indirection,
        );
        let fixed_metadata_bytes = checked_ledger_sum(&[
            grid_capacity,
            sparse.fixed_metadata_bytes(),
            descriptor_indices.size(),
            light_scale.size(),
        ])?;
        let active_capacity_bytes = sparse.active_capacity_bytes();
        let limits = device.limits();
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
            descriptor_indices,
            light_scale,
            last_light_scale: initial_light_scale,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        intermediate_sampled: &wgpu::TextureView,
        output_storage: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        grid_buffer: &wgpu::Buffer,
        sparse: &StreamingSparseBuffers,
        animation_descriptors: &wgpu::Buffer,
        animation_samples: &wgpu::Buffer,
        descriptor_indices: &wgpu::Buffer,
        light_scale: &wgpu::Buffer,
        compose_indirection: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Streamed Animated Direct SH Bind Group"),
            layout,
            entries: &[
                texture_entry(0, intermediate_sampled),
                storage_texture_entry(1, output_storage),
                sampler_entry(BIND_BASE_SAMPLER, sampler),
                dynamic_grid_entry(grid_buffer),
                storage_entry(BIND_DELTA_SUBBLOCKS, sparse.tile_words()),
                storage_entry(BIND_AFFINITY_OFFSETS, sparse.row_pairs()),
                storage_entry(BIND_ANIMATION_DESCRIPTORS, animation_descriptors),
                storage_entry(BIND_ANIMATION_SAMPLES, animation_samples),
                storage_entry(BIND_AFFINITY_LIGHTS, sparse.lights()),
                storage_entry(BIND_ANIMATION_DESCRIPTOR_INDICES, descriptor_indices),
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
    queue: &wgpu::Queue,
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
    timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
) -> Result<(), ShResidencyDrainError> {
    let upload = build_dynamic_compose_grid_upload_for_rows(
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
    if upload.dispatches.is_empty() {
        return Ok(());
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
    Ok(())
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
            super::WGSL_DECODE_HELPER,
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
