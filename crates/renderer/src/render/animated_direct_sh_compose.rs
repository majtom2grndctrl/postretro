// Animated-direct SH addition pass. Kept separate from promotion composition so
// section-45 resources and bindings stay isolated from the legacy Case 1 path.

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_render_cpu::sh_compose::{
    ComposeGridParams, build_animated_direct_delta_buffers, build_compose_grid_bytes,
    pad_storage_bytes, u16_slice_to_bytes, u32_slice_to_bytes,
};

use super::direct_sh_compose::{
    BIND_AFFINITY_LIGHTS, BIND_AFFINITY_OFFSETS, BIND_ANIMATION_DESCRIPTOR_INDICES,
    BIND_ANIMATION_DESCRIPTORS, BIND_ANIMATION_SAMPLES, BIND_BASE_SAMPLER, BIND_DELTA_SUBBLOCKS,
    DirectComposeLayout, nearest_sampler, sampler_bgl_entry, storage_bgl_entry,
    storage_texture_bgl_entry, texture_bgl_entry, uniform_bgl_entry,
};
use super::sh_volume::ShVolumeResources;

pub(super) struct AnimatedDirectShComposePipeline {
    pub(super) pipeline: wgpu::ComputePipeline,
    pub(super) bind_group: wgpu::BindGroup,
}

pub(super) fn build_animated_direct_pass(
    device: &wgpu::Device,
    sh: &ShVolumeResources,
    layout: DirectComposeLayout,
    animated_delta: &AnimatedDirectShDeltaVolumesSection,
    intermediate_sampled_view: &wgpu::TextureView,
    output_storage_view: &wgpu::TextureView,
    uniform_bind_group_layout: &wgpu::BindGroupLayout,
) -> AnimatedDirectShComposePipeline {
    let buffers = build_animated_direct_delta_buffers(Some(animated_delta), layout.grid_dimensions);
    let subblock_bytes = pad_storage_bytes(u16_slice_to_bytes(&buffers.delta_subblocks), 4);
    let offsets_bytes = pad_storage_bytes(u32_slice_to_bytes(&buffers.affinity_offsets), 8);
    let lights_bytes = pad_storage_bytes(u32_slice_to_bytes(&buffers.affinity_lights), 4);
    let descriptor_indices_bytes =
        pad_storage_bytes(u32_slice_to_bytes(&buffers.animation_descriptor_indices), 4);

    use wgpu::util::DeviceExt;
    let delta_subblocks_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Animated Direct SH Compose Delta Subblocks (f16)"),
        contents: &subblock_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let affinity_offsets_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Animated Direct SH Compose Affinity Offsets"),
        contents: &offsets_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let affinity_lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Animated Direct SH Compose Affinity Lights"),
        contents: &lights_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let descriptor_indices_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Animated Direct SH Compose Descriptor Indices"),
        contents: &descriptor_indices_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let grid_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Animated Direct SH Compose Grid Dims"),
        contents: &build_compose_grid_bytes(ComposeGridParams {
            grid_dimensions: layout.grid_dimensions,
            atlas_dimensions: layout.atlas_dimensions,
            tile_dimension: layout.tile_dimension,
            tile_border: layout.tile_border,
            atlas_tiles_per_row: layout.atlas_tiles_per_row,
            tiles_per_layer: layout.tiles_per_layer,
            atlas_layer_count: layout.atlas_layer_count,
            affinity_dims: buffers.affinity_dims,
        }),
        usage: wgpu::BufferUsages::UNIFORM,
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Animated Direct SH Compose BGL"),
        entries: &animated_compose_bgl_entries(),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Animated Direct SH Compose Pipeline Layout"),
        bind_group_layouts: &[Some(uniform_bind_group_layout), Some(&bind_group_layout)],
        immediate_size: 0,
    });
    let shader_source = concat!(
        include_str!("../shaders/animated_direct_sh_compose.wgsl"),
        "\n",
        include_str!("../shaders/curve_eval.wgsl"),
    );
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Animated Direct SH Compose Shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("Animated Direct SH Compose Pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("animated_compose_main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });

    let intermediate_sampler =
        nearest_sampler(device, "Animated Direct SH Compose Intermediate Sampler");
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Animated Direct SH Compose Bind Group"),
        layout: &bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(intermediate_sampled_view),
            },
            wgpu::BindGroupEntry {
                binding: BIND_BASE_SAMPLER,
                resource: wgpu::BindingResource::Sampler(&intermediate_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(output_storage_view),
            },
            wgpu::BindGroupEntry {
                binding: 18,
                resource: grid_buffer.as_entire_binding(),
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
                binding: BIND_ANIMATION_DESCRIPTORS,
                resource: sh.animation.descriptors.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_ANIMATION_SAMPLES,
                resource: sh.animation.anim_samples.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_AFFINITY_LIGHTS,
                resource: affinity_lights_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_ANIMATION_DESCRIPTOR_INDICES,
                resource: descriptor_indices_buffer.as_entire_binding(),
            },
        ],
    });

    AnimatedDirectShComposePipeline {
        pipeline,
        bind_group,
    }
}

fn animated_compose_bgl_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        texture_bgl_entry(0),
        sampler_bgl_entry(BIND_BASE_SAMPLER),
        storage_texture_bgl_entry(1),
        uniform_bgl_entry(18),
        storage_bgl_entry(BIND_DELTA_SUBBLOCKS),
        storage_bgl_entry(BIND_AFFINITY_OFFSETS),
        storage_bgl_entry(BIND_ANIMATION_DESCRIPTORS),
        storage_bgl_entry(BIND_ANIMATION_SAMPLES),
        storage_bgl_entry(BIND_AFFINITY_LIGHTS),
        storage_bgl_entry(BIND_ANIMATION_DESCRIPTOR_INDICES),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animated_pass_uses_exactly_six_storage_buffers() {
        let storage_bindings: Vec<u32> = animated_compose_bgl_entries()
            .into_iter()
            .filter_map(|entry| {
                matches!(
                    entry.ty,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { .. },
                        ..
                    }
                )
                .then_some(entry.binding)
            })
            .collect();
        assert_eq!(
            storage_bindings,
            vec![
                BIND_DELTA_SUBBLOCKS,
                BIND_AFFINITY_OFFSETS,
                BIND_ANIMATION_DESCRIPTORS,
                BIND_ANIMATION_SAMPLES,
                BIND_AFFINITY_LIGHTS,
                BIND_ANIMATION_DESCRIPTOR_INDICES,
            ]
        );
    }

    #[test]
    fn shader_parses_and_exports_pass_b() {
        let source = concat!(
            include_str!("../shaders/animated_direct_sh_compose.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
        );
        let module = naga::front::wgsl::parse_str(source)
            .expect("animated direct compose shader should parse as WGSL");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("animated direct compose shader should validate");
        assert!(module.entry_points.iter().any(|entry| {
            entry.name == "animated_compose_main" && entry.stage == naga::ShaderStage::Compute
        }));
    }
}
