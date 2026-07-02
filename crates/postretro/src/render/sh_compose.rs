// SH compose compute pass: merges the static base octahedral irradiance atlas
// with animated per-light delta tiles into the total atlas consumed by samplers.
// See: context/lib/rendering_pipeline.md §7.1

use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_render_cpu::sh_compose::{
    ComposeStorageFootprint, build_compose_grid_bytes, build_delta_buffers, pad_storage_bytes,
    u16_slice_to_bytes, u32_slice_to_bytes,
};

use super::sh_volume::{ANIMATION_DESCRIPTOR_SIZE, AnimatedLightBuffers, ShVolumeResources};

// SH Compose Bind Group (`@group(1)`) binding index assignments. The shader
// mirrors these (changing either requires updating both).
//
//   @group(1):
//     0      base octahedral atlas        (sampled)
//     1      total octahedral atlas       (storage write)
//     18     GridDims uniform             (atlas/grid/tile/affinity mapping)
//     19     GridOrigin uniform           (grid_origin + cell_size)
//     20     delta_subblocks  (storage)   f16 payload, raw `u16` halves; shader `unpack2x16float`s
//     21     affinity_offsets (storage)   `u32` CSR offsets (affinity_cell_count + 1)
//     22     animation descriptors        (storage, shared with the SH bind group)
//     23     animation samples            (storage, shared with the SH bind group)
//     24     affinity_lights  (storage)   `u32` flat light indices, CSR-parallel to delta subblocks
//     25     animation descriptor indices `u32` delta-light index → descriptor slot
//
// 20/21 replace the old dense per-light `DeltaLightMeta`/`delta_probes` pair.
// 24 is numbered after the shared 22/23 so adding `affinity_lights` doesn't
// renumber the animation bindings shared with the SH bind group.

const BIND_DELTA_SUBBLOCKS: u32 = 20;
const BIND_AFFINITY_OFFSETS: u32 = 21;
const BIND_AFFINITY_LIGHTS: u32 = 24;
const BIND_ANIMATION_DESCRIPTOR_INDICES: u32 = 25;
/// GPU-side compose pass. Always present — levels without an SH section get
/// dummy 1×1 octahedral atlases plus valid zeroed depth-moment resources and a
/// single workgroup dispatch. Unconditional dispatch avoids branching in the
/// frame loop.
pub struct ShComposeResources {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    /// Atlas dimensions. Drives the dispatch shape — one thread per atlas
    /// texel, rounded up to the shader's 8×8 workgroup size.
    dispatch_dimensions: [u32; 2],
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
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        // Build the sparse CSR delta buffers. Probes stay f16 (raw `u16` halves)
        // in the storage buffer — the shader `unpack2x16float`s them. No
        // f16→f32 expansion.
        let buffers = build_delta_buffers(delta, sh.grid_dimensions);
        let light_count = buffers.animated_light_count;

        // wgpu rejects zero-sized storage buffers; pad each to a minimum size so
        // the bind group is always valid. The shader's per-cell loop runs zero
        // times when `affinity_offsets[cell] == affinity_offsets[cell + 1]`, so
        // the padded `delta_subblocks`/`affinity_lights` contents are never read.
        //
        // `affinity_offsets` is the exception: the shader reads both
        // `affinity_offsets[cell]` and `affinity_offsets[cell + 1]` before
        // entering the loop, so the empty case must pad to two `u32`s (8 bytes).
        // Both are zero, so `start == end` and the loop skips — but `[0]` and
        // `[1]` are genuinely in bounds rather than relying on OOB clamping.
        let subblock_bytes = pad_storage_bytes(u16_slice_to_bytes(&buffers.delta_subblocks), 4);
        let offsets_bytes = pad_storage_bytes(u32_slice_to_bytes(&buffers.affinity_offsets), 8);
        let lights_bytes = pad_storage_bytes(u32_slice_to_bytes(&buffers.affinity_lights), 4);
        let descriptor_index_bytes =
            pad_storage_bytes(u32_slice_to_bytes(&buffers.animation_descriptor_indices), 4);

        use wgpu::util::DeviceExt;
        let delta_subblocks_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH Compose Delta Subblocks (f16)"),
            contents: &subblock_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let affinity_offsets_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("SH Compose Affinity Offsets"),
                contents: &offsets_bytes,
                usage: wgpu::BufferUsages::STORAGE,
            });
        let affinity_lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH Compose Affinity Lights"),
            contents: &lights_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let animation_descriptor_indices_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("SH Compose Animation Descriptor Indices"),
                contents: &descriptor_index_bytes,
                usage: wgpu::BufferUsages::STORAGE,
            });

        // Footprint AC: report per-binding byte sizes of every `@group(1)`
        // storage buffer the compose pass binds, plus the combined total. The
        // CSR form should keep this well under the storage-buffer binding floor
        // regardless of animated-light count.
        let footprint = ComposeStorageFootprint {
            delta_subblocks_bytes: subblock_bytes.len(),
            affinity_offsets_bytes: offsets_bytes.len(),
            affinity_lights_bytes: lights_bytes.len(),
            animation_descriptor_indices_bytes: descriptor_index_bytes.len(),
        };
        footprint.log();

        let grid_bytes = build_compose_grid_bytes(
            sh.grid_dimensions,
            sh.atlas_dimensions,
            sh.tile_dimension,
            sh.tile_border,
            sh.atlas_tiles_per_row,
            buffers.affinity_dims,
        );
        let grid_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH Compose Grid Dims"),
            contents: &grid_bytes[..],
            usage: wgpu::BufferUsages::UNIFORM,
        });

        // Grid origin uniform: vec3<f32> grid_origin, f32 _pad, vec3<f32> cell_size, f32 _pad.
        // Retained at binding 19 so the compose bind layout stays compatible with
        // the broader renderer resource setup; the atlas compose path does not
        // need world-space reconstruction.
        let (grid_origin, cell_size) = match sh_section {
            Some(s) => (s.grid_origin, s.cell_size),
            None => ([0.0; 3], [1.0; 3]),
        };
        let mut origin_bytes = [0u8; 32];
        origin_bytes[0..4].copy_from_slice(&grid_origin[0].to_ne_bytes());
        origin_bytes[4..8].copy_from_slice(&grid_origin[1].to_ne_bytes());
        origin_bytes[8..12].copy_from_slice(&grid_origin[2].to_ne_bytes());
        origin_bytes[16..20].copy_from_slice(&cell_size[0].to_ne_bytes());
        origin_bytes[20..24].copy_from_slice(&cell_size[1].to_ne_bytes());
        origin_bytes[24..28].copy_from_slice(&cell_size[2].to_ne_bytes());
        let origin_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH Compose Grid Origin"),
            contents: &origin_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
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
        let shader_source = concat!(
            include_str!("../shaders/sh_compose.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
        );
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
                binding: 18,
                resource: grid_buffer.as_entire_binding(),
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

        // Keep the `AnimatedLightBuffers` import live; the type is borrowed
        // via bind group entries above, not held directly.
        let _ = std::marker::PhantomData::<AnimatedLightBuffers>;
        let _ = ANIMATION_DESCRIPTOR_SIZE;

        log::info!(
            "[Renderer] SH compose: base grid {}×{}×{}, {} animated delta light(s)",
            sh.grid_dimensions[0],
            sh.grid_dimensions[1],
            sh.grid_dimensions[2],
            light_count,
        );

        Self {
            pipeline,
            bind_group,
            dispatch_dimensions: sh.atlas_dimensions,
        }
    }

    /// Encode the per-frame compose dispatch. The accumulated animated delta is
    /// always added to the base at full weight (the `delta_scale` knob was
    /// retired with the indirect-only delta).
    pub fn dispatch(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("SH Compose"),
            timestamp_writes,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, uniform_bind_group, &[]);
        pass.set_bind_group(1, &self.bind_group, &[]);
        let wg_x = self.dispatch_dimensions[0].div_ceil(8).max(1);
        let wg_y = self.dispatch_dimensions[1].div_ceil(8).max(1);
        let wg_z = 1;
        pass.dispatch_workgroups(wg_x, wg_y, wg_z);
    }
}

fn compose_bgl_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
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
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        },
        // Binding 18: atlas/grid/tile/affinity mapping.
        wgpu::BindGroupLayoutEntry {
            binding: 18,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
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
    #[test]
    fn sh_compose_shader_parses_and_exports_compose_main() {
        // curve_eval.wgsl must be appended to resolve Catmull-Rom helpers.
        let src = concat!(
            include_str!("../shaders/sh_compose.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
        );
        let module =
            naga::front::wgsl::parse_str(src).expect("sh_compose.wgsl should parse as WGSL");
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
}
