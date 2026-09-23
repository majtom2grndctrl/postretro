// SH compose compute pass: merges the static base octahedral irradiance atlas
// with animated per-light delta tiles into the total atlas consumed by samplers.
// See: context/lib/rendering_pipeline.md §7.1

use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_render_cpu::frame_uniforms::LightTermMask;
#[cfg(feature = "dev-tools")]
use postretro_render_cpu::sh_compose::ComposeStorageFootprint;
use postretro_render_cpu::sh_compose::{
    ComposeGridParams, DYNAMIC_COMPOSE_GRID_DIMS_SIZE, build_delta_buffers,
};
use postretro_render_cpu::sh_volume::LEGACY_SH_PHYSICAL_TILE_STRIDE;

use super::sh_allocation::{
    ShAllocationKind, buffer_allocation, compose_origin_bytes, compose_storage_payloads,
    probe_indirection_storage_payload,
};
#[cfg(test)]
use super::sh_compose_dispatch::should_dispatch as indirect_compose_should_dispatch;
use super::sh_compose_dispatch::{
    DynamicComposeDispatch, build_dynamic_compose_grid_upload, should_dispatch,
};
use super::sh_indirection::WGSL_DECODE_HELPER;
use super::sh_residency::{ShAllocationLedger, ShResidencyAllocationState, source_ids};
use super::sh_volume::{AnimatedLightBuffers, ShVolumeResources};

// SH Compose Bind Group (`@group(1)`) binding index assignments. The shader
// mirrors these (changing either requires updating both).
//
//   @group(1):
//     0      base octahedral atlas        (sampled)
//     1      total octahedral atlas       (storage write)
//     18     GridDims uniform             (atlas/grid/tile/affinity mapping)
//     19     GridOrigin uniform           (grid_origin + cell_size)
//     20     delta_subblocks  (storage)   f16 payload, raw `u16` halves; shader `unpack2x16float`s
//     21     affinity_offsets (storage)   `(start, end)` u32 pair per affinity row
//     22     animation descriptors        (storage, shared with the SH bind group)
//     23     animation samples            (storage, shared with the SH bind group)
//     24     affinity_lights  (storage)   `u32` flat light indices, CSR-parallel to delta subblocks
//     25     animation descriptor indices `u32` delta-light index → descriptor slot
//     26     probe indirection (storage)  compact valid-probe slot, or invalid sentinel
//     27     delta compaction metadata (storage)  cell masks + levels + post-drop entry offsets
//
// 20/21 replace the old dense per-light `DeltaLightMeta`/`delta_probes` pair.
// 24 is numbered after the shared 22/23 so adding `affinity_lights` doesn't
// renumber the animation bindings shared with the SH bind group.

const BIND_DELTA_SUBBLOCKS: u32 = 20;
const BIND_AFFINITY_OFFSETS: u32 = 21;
const BIND_AFFINITY_LIGHTS: u32 = 24;
const BIND_ANIMATION_DESCRIPTOR_INDICES: u32 = 25;
const BIND_PROBE_INDIRECTION: u32 = 26;
const BIND_DELTA_COMPACTION_META: u32 = 27;
const BIND_BASE_ATLAS_SAMPLER: u32 = 2;
/// GPU-side compose pass. Always present — levels without an SH section get
/// dummy 1×1 octahedral atlases plus valid zeroed depth-moment resources and a
/// valid one-workgroup copy-through dispatch. After that initial pass, the
/// full-grid compose is gated on animated-descriptor activity or a frame-mask
/// change; when it dispatches, it still performs no per-cell or per-tile cull.
pub struct ShComposeResources {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    /// Adapter-bounded rows of the flattened affinity grid. Each selects a
    /// matching dynamically-offset 80-byte grid record at binding 18.
    dispatches: Vec<DynamicComposeDispatch>,
    /// Per-delta-light map to the shared animated-light descriptor slot.
    /// This is the same list bound at binding 25.
    animation_descriptor_indices: Vec<u32>,
    /// The total atlas starts zeroed and must receive the base copy before its
    /// first world-frame sample.
    pending_copy_through: bool,
    /// Retains one base-only dispatch after animated activity ends.
    was_active: bool,
    /// Frame mask that produced the current total atlas.
    last_composed_mask: LightTermMask,
}

impl ShComposeResources {
    /// Build the compose pipeline and bind group. When `delta` is `None` or
    /// empty, all CSR offset ranges are empty (`start == end`), so the result is
    /// a pure base→total copy.
    pub fn new(
        device: &wgpu::Device,
        sh: &ShVolumeResources,
        sh_section: Option<&postretro_level_format::sh_volume::OctahedralShVolumeSection>,
        delta: Option<&DeltaShVolumesSection>,
        sh_section_present: bool,
        delta_section_present: bool,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        ledger: &mut ShAllocationLedger,
    ) -> Self {
        // Build the sparse CSR metadata. Probes stay in the format-owned f16
        // payload until this renderer stages its verbatim bytes for upload.
        let delta_subblocks: &[u16] = delta.map_or(&[], |delta| delta.delta_subblocks.as_slice());
        let buffers = build_delta_buffers(delta, sh.grid_dimensions);
        let light_count = buffers.animated_light_count;

        // wgpu rejects zero-sized storage buffers; pad each to a minimum size so
        // the bind group is always valid. The shader's per-cell loop runs zero
        // times when a row's `start == end`, so
        // the padded `delta_subblocks`/`affinity_lights` contents are never read.
        //
        // `affinity_offsets` is the exception: the shader reads a `(start, end)`
        // pair before entering the loop, so the empty case must pad to two `u32`s
        // (8 bytes). Both are zero and genuinely in bounds rather than relying
        // on OOB clamping.
        let storage = compose_storage_payloads(
            ShAllocationKind::IndirectComposeDeltaSubblocks,
            ShAllocationKind::IndirectComposeCompactionMetadata,
            ShAllocationKind::IndirectComposeAffinityOffsets,
            ShAllocationKind::IndirectComposeAffinityLights,
            Some(ShAllocationKind::IndirectComposeDescriptorIndices),
            delta_subblocks,
            &buffers.compaction_meta_words(),
            &buffers.affinity_offsets,
            &buffers.affinity_lights,
            Some(&buffers.animation_descriptor_indices),
        );
        // ShVolumeResources derives this once from id-34 metadata. The direct
        // compose carriers and B/A moment payload use the exact same words.
        let probe_indirection = probe_indirection_storage_payload(
            ShAllocationKind::IndirectComposeProbeIndirection,
            &sh.probe_indirection_words,
        );
        let delta_sources = source_ids([delta_section_present.then_some(27)]);
        let compose_sources = source_ids([
            sh_section_present.then_some(34),
            delta_section_present.then_some(27),
        ]);
        let delta_state = match (delta_section_present, delta.is_some()) {
            (true, true) => ShResidencyAllocationState::Data,
            (true, false) => ShResidencyAllocationState::Fallback,
            (false, _) => ShResidencyAllocationState::Dummy,
        };
        let sh_state = match (sh_section_present, sh.present) {
            (true, true) => ShResidencyAllocationState::Data,
            (true, false) => ShResidencyAllocationState::Fallback,
            (false, _) => ShResidencyAllocationState::Dummy,
        };
        let compose_state = if sh.present {
            ShResidencyAllocationState::Data
        } else if sh_section_present || delta_section_present {
            ShResidencyAllocationState::Fallback
        } else {
            ShResidencyAllocationState::Dummy
        };
        ledger.record_buffer(
            storage.delta_subblocks.allocation,
            &delta_sources,
            !delta_section_present,
            delta_state,
        );
        ledger.record_buffer(
            storage.compaction_metadata.allocation,
            &delta_sources,
            !delta_section_present,
            delta_state,
        );
        ledger.record_buffer(
            storage.affinity_offsets.allocation,
            &delta_sources,
            !delta_section_present,
            delta_state,
        );
        ledger.record_buffer(
            storage.affinity_lights.allocation,
            &delta_sources,
            !delta_section_present,
            delta_state,
        );
        ledger.record_buffer(
            storage
                .descriptor_indices
                .as_ref()
                .expect("indirect compose always describes descriptor indices")
                .allocation,
            &delta_sources,
            !delta_section_present,
            delta_state,
        );
        ledger.record_buffer(
            probe_indirection.allocation,
            &source_ids([sh_section_present.then_some(34)]),
            !sh_section_present,
            sh_state,
        );

        use wgpu::util::DeviceExt;
        let delta_subblocks_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH Compose Delta Subblocks (f16)"),
            contents: &storage.delta_subblocks.contents,
            usage: storage.delta_subblocks.allocation.usage,
        });
        let affinity_offsets_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("SH Compose Affinity Offsets"),
                contents: &storage.affinity_offsets.contents,
                usage: storage.affinity_offsets.allocation.usage,
            });
        let affinity_lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH Compose Affinity Lights"),
            contents: &storage.affinity_lights.contents,
            usage: storage.affinity_lights.allocation.usage,
        });
        let animation_descriptor_indices_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("SH Compose Animation Descriptor Indices"),
                contents: &storage
                    .descriptor_indices
                    .as_ref()
                    .expect("indirect compose always describes descriptor indices")
                    .contents,
                usage: storage
                    .descriptor_indices
                    .as_ref()
                    .expect("indirect compose always describes descriptor indices")
                    .allocation
                    .usage,
            });
        let probe_indirection_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("SH Compose Probe Indirection"),
                contents: &probe_indirection.contents,
                usage: probe_indirection.allocation.usage,
            });
        let delta_compaction_meta_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("SH Compose Delta Compaction Meta"),
                contents: &storage.compaction_metadata.contents,
                usage: storage.compaction_metadata.allocation.usage,
            });

        // Report per-binding byte sizes only in development builds. The CSR
        // form should keep this well under the storage-buffer binding floor
        // regardless of animated-light count.
        #[cfg(feature = "dev-tools")]
        let footprint = ComposeStorageFootprint {
            delta_subblocks_bytes: storage.delta_subblocks.allocation.byte_len,
            delta_compaction_meta_bytes: storage.compaction_metadata.allocation.byte_len,
            affinity_offsets_bytes: storage.affinity_offsets.allocation.byte_len,
            affinity_lights_bytes: storage.affinity_lights.allocation.byte_len,
            animation_descriptor_indices_bytes: storage
                .descriptor_indices
                .as_ref()
                .expect("indirect compose always describes descriptor indices")
                .allocation
                .byte_len,
        };
        #[cfg(feature = "dev-tools")]
        footprint.log("SH compose @group(1)");

        let device_limits = device.limits();
        let grid_upload = build_dynamic_compose_grid_upload(
            ComposeGridParams {
                grid_dimensions: sh.grid_dimensions,
                atlas_dimensions: sh.atlas_dimensions,
                tile_dimension: sh.tile_dimension,
                tile_border: sh.tile_border,
                atlas_tiles_per_row: sh.atlas_tiles_per_row,
                tiles_per_layer: sh.tiles_per_layer,
                atlas_layer_count: sh.atlas_layer_count,
                affinity_dims: buffers.affinity_dims,
                compact_atlas_tiles_per_row: sh_section
                    .map(|section| section.atlas_tiles_per_row)
                    .unwrap_or(1),
                compact_atlas_tiles_per_layer: sh_section
                    .map(|section| section.tiles_per_layer)
                    .unwrap_or(1),
            },
            LEGACY_SH_PHYSICAL_TILE_STRIDE,
            device_limits.max_compute_workgroups_per_dimension,
            device_limits.min_uniform_buffer_offset_alignment,
            device_limits.max_buffer_size,
        )
        .expect("validated SH affinity dimensions must fit adapter-bounded compose ranges");
        let grid_allocation = buffer_allocation(
            ShAllocationKind::IndirectComposeGrid,
            &grid_upload.bytes,
            wgpu::BufferUsages::UNIFORM,
        );
        ledger.record_buffer(
            grid_allocation,
            &compose_sources,
            compose_sources.is_empty(),
            compose_state,
        );
        let grid_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH Compose Grid Dims"),
            contents: &grid_upload.bytes,
            usage: grid_allocation.usage,
        });

        // Grid origin uniform: vec3<f32> grid_origin, f32 _pad, vec3<f32> cell_size, f32 _pad.
        // Retained at binding 19 so the compose bind layout stays compatible with
        // the broader renderer resource setup; the atlas compose path does not
        // need world-space reconstruction.
        let (grid_origin, cell_size) = match sh_section {
            Some(s) => (s.grid_origin, s.cell_size),
            None => ([0.0; 3], [1.0; 3]),
        };
        let origin_bytes = compose_origin_bytes(grid_origin, cell_size);
        let origin_allocation = buffer_allocation(
            ShAllocationKind::IndirectComposeOrigin,
            &origin_bytes,
            wgpu::BufferUsages::UNIFORM,
        );
        ledger.record_buffer(
            origin_allocation,
            &compose_sources,
            compose_sources.is_empty(),
            compose_state,
        );
        let origin_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH Compose Grid Origin"),
            contents: &origin_bytes,
            usage: origin_allocation.usage,
        });

        // Build the bind group layout + pipeline.
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SH Compose BGL"),
            entries: &compose_bgl_entries(),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("SH Compose Pipeline Layout"),
            bind_group_layouts: &[Some(uniform_bind_group_layout), Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // curve_eval.wgsl provides `sample_curve_catmull_rom` used by the shader.
        let shader_source = [
            include_str!("../shaders/sh_compose.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
            "\n",
            WGSL_DECODE_HELPER,
        ]
        .concat();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SH Compose Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("SH Compose Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("compose_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        // BC6H cannot use textureLoad on Metal. Compose reads its immutable
        // base at exact texel centers through this nearest, non-filtering
        // sampler; it is deliberately local to this compose bind group.
        let base_atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("SH Compose Base Atlas Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let entries: Vec<wgpu::BindGroupEntry> = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&sh.base_atlas_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&sh.total_atlas_storage_view),
            },
            wgpu::BindGroupEntry {
                binding: BIND_BASE_ATLAS_SAMPLER,
                resource: wgpu::BindingResource::Sampler(&base_atlas_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 18,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &grid_buffer,
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
                resource: delta_subblocks_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_AFFINITY_OFFSETS,
                resource: affinity_offsets_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_AFFINITY_LIGHTS,
                resource: affinity_lights_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_ANIMATION_DESCRIPTOR_INDICES,
                resource: animation_descriptor_indices_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_PROBE_INDIRECTION,
                resource: probe_indirection_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_DELTA_COMPACTION_META,
                resource: delta_compaction_meta_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 22,
                resource: sh.animation.descriptors.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 23,
                resource: sh.animation.anim_samples.as_entire_binding(),
            },
        ];

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SH Compose Bind Group"),
            layout: &bind_group_layout,
            entries: &entries,
        });

        log::info!(
            "[Renderer] SH compose: base grid {}×{}×{}, {} animated delta light(s)",
            sh.grid_dimensions[0],
            sh.grid_dimensions[1],
            sh.grid_dimensions[2],
            light_count,
        );

        let animation_descriptor_indices = buffers.animation_descriptor_indices;
        Self {
            pipeline,
            bind_group,
            dispatches: grid_upload.dispatches,
            animation_descriptor_indices,
            pending_copy_through: true,
            was_active: false,
            last_composed_mask: LightTermMask::ALL,
        }
    }

    /// Whether any delta light consumed by this pass is active in the shared
    /// animation descriptor mirror.
    pub fn has_active_animated_descriptor(&self, animation: &AnimatedLightBuffers) -> bool {
        animation.any_active_for_descriptor_indices(&self.animation_descriptor_indices)
    }

    /// Encode a compose dispatch only when the full composed atlas could have
    /// changed. The accumulated animated delta is always added to the base at
    /// full weight (the `delta_scale` knob was retired with the indirect-only
    /// delta).
    pub fn dispatch_if_needed(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        active: bool,
        frame_light_term_mask: LightTermMask,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        if !should_dispatch(
            active,
            self.pending_copy_through,
            self.was_active,
            frame_light_term_mask,
            self.last_composed_mask,
        ) {
            return;
        }

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("SH Compose"),
                timestamp_writes,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, uniform_bind_group, &[]);
            for dispatch in &self.dispatches {
                pass.set_bind_group(1, &self.bind_group, &[dispatch.dynamic_offset]);
                pass.dispatch_workgroups(dispatch.workgroup_count, 1, 1);
            }
        }

        self.pending_copy_through = false;
        self.was_active = active;
        self.last_composed_mask = frame_light_term_mask;
    }
}

fn compose_bgl_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba16Float,
                view_dimension: wgpu::TextureViewDimension::D2Array,
            },
            count: None,
        },
        // Binding 2: nearest sampler for the compact BC6H/RGBA16F base atlas.
        wgpu::BindGroupLayoutEntry {
            binding: BIND_BASE_ATLAS_SAMPLER,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
            count: None,
        },
        // Binding 18: atlas/grid/tile/affinity mapping.
        wgpu::BindGroupLayoutEntry {
            binding: 18,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(DYNAMIC_COMPOSE_GRID_DIMS_SIZE as u64),
            },
            count: None,
        },
        // Binding 19: grid_origin + cell_size.
        wgpu::BindGroupLayoutEntry {
            binding: 19,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // Binding 20: delta_subblocks — sparse f16 probe payload (raw `u16` halves).
        wgpu::BindGroupLayoutEntry {
            binding: BIND_DELTA_SUBBLOCKS,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // Binding 21: affinity_offsets — CSR offsets (`u32`).
        wgpu::BindGroupLayoutEntry {
            binding: BIND_AFFINITY_OFFSETS,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // Binding 24: affinity_lights — flat CSR light indices (`u32`). Numbered
        // after the shared 22/23 so those keep their indices.
        wgpu::BindGroupLayoutEntry {
            binding: BIND_AFFINITY_LIGHTS,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // Binding 25: delta-light index → animation descriptor slot.
        wgpu::BindGroupLayoutEntry {
            binding: BIND_ANIMATION_DESCRIPTOR_INDICES,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // Binding 26: dense-probe index -> compact id-34 tile slot, or the
        // invalid sentinel. Binding 27 packs id-27 valid-probe masks and
        // post-drop entry offsets. Together they are the eighth compute-visible
        // storage buffer, exactly at the downlevel ceiling.
        wgpu::BindGroupLayoutEntry {
            binding: BIND_PROBE_INDIRECTION,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: BIND_DELTA_COMPACTION_META,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // Bindings 22–23: animation descriptors and samples (shared with SH bind group).
        wgpu::BindGroupLayoutEntry {
            binding: 22,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 23,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::super::sh_indirection::{INVALID_PROBE_INDIRECTION, build_probe_indirection_words};
    use super::{
        BIND_BASE_ATLAS_SAMPLER, build_delta_buffers, compose_bgl_entries,
        indirect_compose_should_dispatch,
    };
    use postretro_level_format::delta_sh_volumes::{AFFINITY_FACTOR, DeltaShVolumesSection};
    use postretro_level_format::octahedral::{
        DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
    };
    use postretro_level_format::sh_volume::{OctahedralShProbe, OctahedralShVolumeSection};
    use postretro_render_cpu::frame_uniforms::LightTermMask;
    use postretro_render_cpu::sh_compose::DYNAMIC_COMPOSE_GRID_DIMS_SIZE;

    #[test]
    fn sh_compose_shader_parses_and_exports_compose_main() {
        // curve_eval.wgsl must be appended to resolve Catmull-Rom helpers.
        let src = [
            include_str!("../shaders/sh_compose.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
            "\n",
            super::WGSL_DECODE_HELPER,
        ]
        .concat();
        let module =
            naga::front::wgsl::parse_str(&src).expect("sh_compose.wgsl should parse as WGSL");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("sh_compose.wgsl should validate");
        let has_compose = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "compose_main" && ep.stage == naga::ShaderStage::Compute);
        assert!(has_compose, "compose_main entry point missing");
    }

    #[test]
    fn coarsened_compose_uses_one_brick_workgroup_and_kept_shared_tiles() {
        let source = include_str!("../shaders/sh_compose.wgsl");

        assert!(
            source.contains("@builtin(workgroup_id) workgroup"),
            "one workgroup must own one affinity brick rather than an atlas texel block"
        );
        assert!(
            source.contains("var<workgroup> shared_kept_tiles"),
            "L1/L2 reconstruction must reuse a workgroup-local kept lattice"
        );
        assert!(
            source.contains("if (level == 0u)"),
            "dense L0 cells must keep their direct-read path instead of loading 64 shared tiles"
        );
        assert!(
            source.contains("if (level == 1u && local_probe_is_kept"),
            "L1 must load only kept local corners"
        );
        assert!(
            source.contains("if (level == 2u)"),
            "L2 must load its single synthesized mean tile"
        );
        assert!(
            source.contains("fn slot_tile_origin(slot: u32)")
                && !source.contains("fn atlas_tile_origin("),
            "compose writes must resolve stored slots rather than dense probe indices"
        );
        assert!(
            source.contains("fn l1_node_corner_probe")
                && source.contains("brick_indirection.scale > 0u")
                && source.contains("stored_indirection = decode_sh_probe_indirection")
                && source.matches("&& node_origin_writer").count() >= 2
                && source.contains(
                    "brick_indirection.level == 2u && local_probe == 0u && node_origin_writer"
                )
                && source.contains("let node_edge = 1u << brick_indirection.scale;")
                && source.contains("select(0.0, 1.0, stored_slot.valid)"),
            "scaled L1/L2 nodes must copy through their stored slots from only the node-origin brick"
        );
    }

    #[test]
    fn compose_reads_group_zero_mask_and_gates_static_and_every_animated_path() {
        let source = include_str!("../shaders/sh_compose.wgsl");

        assert!(
            source.contains("let use_indirect_static = (uniforms.light_term_mask & 0x02u) != 0u;")
                && source.contains("if (output_is_stored && use_indirect_static)"),
            "the static base must be selected from the live group-0 mask"
        );
        assert!(
            source
                .contains("let use_indirect_animated = (uniforms.light_term_mask & 0x04u) != 0u;"),
            "the animated delta must read the same live group-0 mask"
        );
        assert_eq!(
            source
                .matches("if (output_is_stored && use_indirect_animated)")
                .count(),
            1,
            "dense L0 delta accumulation must be gated per valid output; coarsened L1/L2 uses the uniform gate asserted below so every workgroup invocation reaches its barriers",
        );
        let coarsened_delta_path = source
            .split("    } else {\n        // Coarsened L1/L2 cells")
            .nth(1)
            .and_then(|path| path.split("\n    if (output_is_stored) {").next())
            .expect("shader must retain its coarsened L1/L2 compose path");
        assert!(
            coarsened_delta_path.contains(
                "if (use_indirect_animated) {\n            for (var entry = start; entry < end; entry = entry + 1u) {"
            ) && coarsened_delta_path.contains("read_delta_texel(")
                && coarsened_delta_path.contains("reconstruct_l1_shared_texel("),
            "the animated-term guard must wrap the coarsened per-entry delta loads and reconstruction loop",
        );
        assert!(
            !source.contains("grid.light_term_mask"),
            "construction-time GridDims must not freeze the per-frame mask",
        );
    }

    #[test]
    fn probe_indirection_uses_metadata_builder_and_zero_invalid_sentinel() {
        let mut section = OctahedralShVolumeSection::placeholder();
        section.grid_dimensions = [5, 1, 1];
        section.probes = vec![
            OctahedralShProbe {
                validity: 0,
                ..Default::default()
            },
            OctahedralShProbe {
                validity: 1,
                ..Default::default()
            },
            OctahedralShProbe {
                validity: 1,
                ..Default::default()
            },
            OctahedralShProbe {
                validity: 0,
                ..Default::default()
            },
            OctahedralShProbe {
                validity: 1,
                ..Default::default()
            },
        ];

        assert_eq!(
            build_probe_indirection_words(Some(&section)),
            vec![
                INVALID_PROBE_INDIRECTION,
                4,
                36,
                INVALID_PROBE_INDIRECTION,
                68
            ],
        );
        assert_eq!(
            build_probe_indirection_words(None),
            vec![INVALID_PROBE_INDIRECTION],
        );
    }

    #[test]
    fn compose_layout_keeps_eight_compute_storage_buffers_and_local_sampler() {
        let entries = compose_bgl_entries();
        let storage_bindings: Vec<_> = entries
            .iter()
            .filter_map(|entry| {
                matches!(
                    entry.ty,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        ..
                    }
                )
                .then_some(entry.binding)
            })
            .collect();
        assert_eq!(storage_bindings, vec![20, 21, 24, 25, 26, 27, 22, 23]);
        assert_eq!(storage_bindings.len(), 8);

        let sampler = entries
            .iter()
            .find(|entry| entry.binding == BIND_BASE_ATLAS_SAMPLER)
            .expect("compose layout should bind its local base-atlas sampler");
        assert!(matches!(
            sampler.ty,
            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering)
        ));

        let grid = entries
            .iter()
            .find(|entry| entry.binding == 18)
            .expect("compose layout should retain GridDims at binding 18");
        let wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset,
            min_binding_size,
        } = &grid.ty
        else {
            panic!("GridDims must remain a uniform buffer");
        };
        assert_eq!(*ty, wgpu::BufferBindingType::Uniform);
        assert!(*has_dynamic_offset);
        assert_eq!(
            min_binding_size.map(std::num::NonZeroU64::get),
            Some(DYNAMIC_COMPOSE_GRID_DIMS_SIZE as u64),
        );
    }

    #[test]
    fn indirect_compose_schedules_load_active_and_zero_transition() {
        assert!(indirect_compose_should_dispatch(
            false,
            true,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(indirect_compose_should_dispatch(
            true,
            false,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(indirect_compose_should_dispatch(
            false,
            false,
            true,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(!indirect_compose_should_dispatch(
            false,
            false,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
    }

    #[test]
    fn indirect_compose_mask_change_and_return_to_all_re_dirty() {
        let mut static_indirect_off = LightTermMask::ALL;
        static_indirect_off.set_enabled(LightTermMask::INDIRECT_STATIC, false);

        assert!(indirect_compose_should_dispatch(
            false,
            false,
            false,
            static_indirect_off,
            LightTermMask::ALL,
        ));
        assert!(indirect_compose_should_dispatch(
            false,
            false,
            false,
            LightTermMask::ALL,
            static_indirect_off,
        ));
    }

    #[test]
    fn indirect_compose_mask_change_stays_dirty_until_a_world_dispatch_records_it() {
        let mut animated_indirect_off = LightTermMask::ALL;
        animated_indirect_off.set_enabled(LightTermMask::INDIRECT_ANIMATED, false);
        let last_composed_mask = LightTermMask::ALL;

        // A non-world frame never calls dispatch_if_needed, so it cannot
        // consume the mask change before the next world frame.
        assert!(indirect_compose_should_dispatch(
            false,
            false,
            false,
            animated_indirect_off,
            last_composed_mask,
        ));
        assert!(indirect_compose_should_dispatch(
            false,
            false,
            false,
            animated_indirect_off,
            last_composed_mask,
        ));
        assert!(!indirect_compose_should_dispatch(
            false,
            false,
            false,
            animated_indirect_off,
            animated_indirect_off,
        ));
    }

    #[test]
    fn empty_indirect_descriptor_indices_leave_compose_idle_after_copy_through() {
        // This is CPU-only coverage for the load-time data feeding binding 25.
        // With no animated indirect delta lights, the shared activity helper
        // receives an empty descriptor-index list and the pass may idle once
        // its initial base copy-through completed.
        let section = DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: Vec::new(),
            valid_probe_masks: vec![0],
            cell_levels: vec![0],
            affinity_offsets: vec![0, 0],
            affinity_lights: Vec::new(),
            delta_subblocks: Vec::new(),
        };

        let buffers = build_delta_buffers(Some(&section), [4, 4, 4]);
        assert_eq!(buffers.animated_light_count, 0);
        assert!(buffers.animation_descriptor_indices.is_empty());
        assert!(!indirect_compose_should_dispatch(
            false,
            false,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
    }
}
