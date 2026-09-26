//! Renderer-owned indirect compose carrier for streamed SH residency.

use super::super::payload::sparse_offsets_fit_allocation;
use super::super::{ParsedSparseRow, SparseInstallPlan};
use super::*;

/// Indirect id-34/id-27 streamed compose. Its buffers are renderer-owned
/// sparse pools; the loader only lends a decoded chunk until the queue writes
/// below have been planned.
pub(super) struct StreamingIndirectCompose {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    grid_buffer: wgpu::Buffer,
    origin_buffer: wgpu::Buffer,
    affinity_offsets: wgpu::Buffer,
    affinity_lights: wgpu::Buffer,
    delta_subblocks: wgpu::Buffer,
    compaction_metadata: wgpu::Buffer,
    descriptor_index_buffer: wgpu::Buffer,
    grid: ComposeGridParams,
    max_workgroups_x: u32,
    dynamic_alignment: u32,
    max_buffer_size: u64,
    grid_capacity: u64,
    entry_capacity: u32,
    tile_f16_capacity: u32,
    animation_descriptor_indices: Vec<u32>,
    source_present: bool,
}

/// The four independently grown id-27 sparse backing buffers retained until
/// their copy submission completes. Pipeline state, bindings, dynamic grids,
/// and all dense resources stay with (or are immediately dropped from) the
/// live compose carrier.
pub(in crate::render::sh_streaming::gpu) struct RetiredIndirectSparseResources {
    #[allow(dead_code)]
    affinity_offsets: wgpu::Buffer,
    #[allow(dead_code)]
    affinity_lights: wgpu::Buffer,
    #[allow(dead_code)]
    delta_subblocks: wgpu::Buffer,
    #[allow(dead_code)]
    compaction_metadata: wgpu::Buffer,
    capacity_bytes: u64,
}

impl RetiredIndirectSparseResources {
    pub(in crate::render::sh_streaming::gpu) const fn capacity_bytes(&self) -> u64 {
        self.capacity_bytes
    }
}

impl StreamingIndirectCompose {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        device: &wgpu::Device,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        shape: AtlasShape,
        base_view: &wgpu::TextureView,
        total_storage_view: &wgpu::TextureView,
        compose_indirection: &wgpu::Buffer,
        sh: &ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        sparse_floor: Option<(u32, u32)>,
    ) -> Result<Self, ShResidencyDrainError> {
        let source = sources.indirect_delta.as_ref();
        let (affinity_dims, masks, levels, descriptor_indices, entry_capacity, tile_f16_capacity) =
            sparse_compose_capacity(base, source, sparse_floor)?;
        let cell_count = checked_cell_count(affinity_dims)?;
        let mut compaction_words = Vec::with_capacity(
            usize::try_from(cell_count)
                .map_err(|_| ShResidencyDrainError::SlotOverflow)?
                .saturating_mul(3)
                .saturating_add(usize::try_from(entry_capacity).unwrap_or(usize::MAX)),
        );
        for mask in masks {
            compaction_words.push(mask as u32);
            compaction_words.push((mask >> 32) as u32);
        }
        compaction_words.extend(levels.into_iter().map(u32::from));
        compaction_words.resize(
            compaction_words
                .len()
                .checked_add(
                    usize::try_from(entry_capacity)
                        .map_err(|_| ShResidencyDrainError::SlotOverflow)?,
                )
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
            0,
        );
        let pair_words = usize::try_from(cell_count)
            .ok()
            .and_then(|count| count.checked_mul(2))
            .ok_or(ShResidencyDrainError::SlotOverflow)?
            .max(2);
        let tile_words = usize::try_from(tile_f16_capacity.div_ceil(2))
            .map_err(|_| ShResidencyDrainError::SlotOverflow)?
            .max(1);
        let entry_words = usize::try_from(entry_capacity)
            .map_err(|_| ShResidencyDrainError::SlotOverflow)?
            .max(1);
        let descriptor_words = if descriptor_indices.is_empty() {
            vec![u32::MAX]
        } else {
            descriptor_indices
        };
        for (byte_len, reason) in [
            (
                u64::try_from(tile_words)
                    .ok()
                    .and_then(|words| words.checked_mul(4))
                    .ok_or(ShResidencyDrainError::SlotOverflow)?,
                "streamed indirect delta tile pool exceeds adapter storage limits",
            ),
            (
                u64::try_from(pair_words)
                    .ok()
                    .and_then(|words| words.checked_mul(4))
                    .ok_or(ShResidencyDrainError::SlotOverflow)?,
                "streamed indirect CSR row-pair table exceeds adapter storage limits",
            ),
            (
                u64::try_from(entry_words)
                    .ok()
                    .and_then(|words| words.checked_mul(4))
                    .ok_or(ShResidencyDrainError::SlotOverflow)?,
                "streamed indirect entry pool exceeds adapter storage limits",
            ),
            (
                u64::try_from(compaction_words.len())
                    .ok()
                    .and_then(|words| words.checked_mul(4))
                    .ok_or(ShResidencyDrainError::SlotOverflow)?,
                "streamed indirect compaction table exceeds adapter storage limits",
            ),
            (
                u64::try_from(descriptor_words.len())
                    .ok()
                    .and_then(|words| words.checked_mul(4))
                    .ok_or(ShResidencyDrainError::SlotOverflow)?,
                "streamed indirect descriptor index table exceeds adapter storage limits",
            ),
        ] {
            validate_storage_buffer_size(device, byte_len, reason)?;
        }

        let grid = ComposeGridParams {
            grid_dimensions: base.grid_dimensions,
            atlas_dimensions: [shape.extent().width, shape.extent().height],
            tile_dimension: base.tile_dimension,
            tile_border: base.tile_border,
            atlas_tiles_per_row: shape.tiles_per_row,
            tiles_per_layer: shape.tiles_per_layer,
            atlas_layer_count: shape.layers,
            affinity_dims,
            // Input and output are the same pooled 8×8 physical-cell layout.
            compact_atlas_tiles_per_row: shape.tiles_per_row,
            compact_atlas_tiles_per_layer: shape.tiles_per_layer,
        };
        let limits = device.limits();
        let rows = checked_cell_count(affinity_dims)?;
        let record_size = u64::try_from(DYNAMIC_COMPOSE_GRID_DIMS_SIZE)
            .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        if record_size > limits.max_uniform_buffer_binding_size {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed compose gather record exceeds uniform binding limit",
            });
        }
        let align = u64::from(limits.min_uniform_buffer_offset_alignment.max(1));
        let record_stride = record_size
            .checked_add(align - 1)
            .and_then(|size| size.checked_div(align))
            .and_then(|count| count.checked_mul(align))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let chunk_capacity = crate::render::sh_compose_dispatch::gather_chunk_capacity(
            limits.max_compute_workgroups_per_dimension,
        )
        .ok_or(ShResidencyDrainError::GpuCapacity {
            reason: "streamed compose gather has zero row capacity",
        })?;
        let chunk_count = rows.max(1).div_ceil(chunk_capacity);
        let grid_capacity = record_stride
            .checked_mul(u64::from(chunk_count))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        if grid_capacity > limits.max_buffer_size {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed dirty compose records exceed adapter buffer limit",
            });
        }
        let grid_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Streamed SH Compose Grid Records"),
            size: grid_capacity.max(record_size),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let origin = compose_origin_bytes(base.grid_origin, base.cell_size);
        let origin_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Streamed SH Compose Grid Origin"),
            contents: &origin,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let delta_subblocks = buffer_with_zeroes(
            device,
            "Streamed SH Indirect Delta Pool",
            tile_words
                .checked_mul(4)
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        );
        let affinity_offsets = buffer_with_zeroes(
            device,
            "Streamed SH Indirect CSR Row Pairs",
            pair_words
                .checked_mul(4)
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        );
        let affinity_lights = buffer_with_zeroes(
            device,
            "Streamed SH Indirect Entry Pool",
            entry_words
                .checked_mul(4)
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        );
        let descriptor_index_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Streamed SH Indirect Descriptor Indices"),
                contents: &u32_bytes(&descriptor_words),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        let compaction_metadata = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Streamed SH Indirect Compaction Metadata"),
            contents: &u32_bytes(&compaction_words),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Streamed SH Compose Base Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Streamed SH Compose BGL"),
            entries: &compose_bgl_entries(),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Streamed SH Compose Pipeline Layout"),
            bind_group_layouts: &[Some(uniform_bind_group_layout), Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let shader_source = [
            include_str!("../../../shaders/sh_compose.wgsl"),
            "\n",
            include_str!("../../../shaders/curve_eval.wgsl"),
            "\n",
            WGSL_DECODE_HELPER,
        ]
        .concat();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Streamed SH Compose Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Streamed SH Compose Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("compose_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let bind_group = Self::build_bind_group(
            device,
            &bind_group_layout,
            base_view,
            total_storage_view,
            &sampler,
            &grid_buffer,
            &origin_buffer,
            &delta_subblocks,
            &affinity_offsets,
            &sh.animation.descriptors,
            &sh.animation.anim_samples,
            &affinity_lights,
            &descriptor_index_buffer,
            compose_indirection,
            &compaction_metadata,
        );
        Ok(Self {
            pipeline,
            bind_group_layout,
            bind_group,
            sampler,
            grid_buffer,
            origin_buffer,
            affinity_offsets,
            affinity_lights,
            delta_subblocks,
            compaction_metadata,
            descriptor_index_buffer,
            grid,
            max_workgroups_x: limits.max_compute_workgroups_per_dimension,
            dynamic_alignment: limits.min_uniform_buffer_offset_alignment,
            max_buffer_size: limits.max_buffer_size,
            grid_capacity,
            entry_capacity,
            tile_f16_capacity,
            animation_descriptor_indices: descriptor_words,
            source_present: source.is_some(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        base_view: &wgpu::TextureView,
        total_storage_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        grid_buffer: &wgpu::Buffer,
        origin_buffer: &wgpu::Buffer,
        delta_subblocks: &wgpu::Buffer,
        affinity_offsets: &wgpu::Buffer,
        animation_descriptors: &wgpu::Buffer,
        animation_samples: &wgpu::Buffer,
        affinity_lights: &wgpu::Buffer,
        descriptor_index_buffer: &wgpu::Buffer,
        compose_indirection: &wgpu::Buffer,
        compaction_metadata: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Streamed SH Compose Bind Group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(base_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(total_storage_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 18,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: grid_buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(DYNAMIC_COMPOSE_GRID_DIMS_SIZE as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 19,
                    resource: origin_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_DELTA_SUBBLOCKS,
                    resource: delta_subblocks.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_AFFINITY_OFFSETS,
                    resource: affinity_offsets.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 22,
                    resource: animation_descriptors.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 23,
                    resource: animation_samples.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_AFFINITY_LIGHTS,
                    resource: affinity_lights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_ANIMATION_DESCRIPTOR_INDICES,
                    resource: descriptor_index_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_PROBE_INDIRECTION,
                    resource: compose_indirection.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_DELTA_COMPACTION_META,
                    resource: compaction_metadata.as_entire_binding(),
                },
            ],
        })
    }
}

mod runtime;
