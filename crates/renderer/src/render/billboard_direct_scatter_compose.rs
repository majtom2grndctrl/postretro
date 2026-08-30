// Animated direct-scatter composition for billboard receivers.
// See: context/lib/rendering_pipeline.md §7.4

use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection;
use postretro_render_cpu::frame_uniforms::LightTermMask;
use postretro_render_cpu::sh_compose::{pad_storage_bytes, u16_slice_to_bytes, u32_slice_to_bytes};
use wgpu::util::DeviceExt;

use super::billboard_direct_scatter::BillboardDirectScatterResources;
use super::sh_volume::AnimatedLightBuffers;

const BIND_BASE: u32 = 0;
const BIND_OUTPUT: u32 = 1;
const BIND_GRID: u32 = 2;
const BIND_DELTAS: u32 = 3;
const BIND_OFFSETS: u32 = 4;
const BIND_DESCRIPTORS: u32 = 5;
const BIND_SAMPLES: u32 = 6;
const BIND_AFFINITY_LIGHTS: u32 = 7;
const BIND_DESCRIPTOR_INDICES: u32 = 8;

struct ComposePipeline {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    dispatch_dimensions: [u32; 3],
    pending_copy_through: bool,
    was_active: bool,
    last_composed_mask: LightTermMask,
}

/// The compose target is allocated only for the validated section-48 path.
/// Static-only section-47 maps bind their immutable base directly and need no
/// per-frame dispatch.
pub(super) struct BillboardDirectScatterComposeResources {
    pipeline: Option<ComposePipeline>,
}

impl BillboardDirectScatterComposeResources {
    pub(super) fn new(
        device: &wgpu::Device,
        scatter: &BillboardDirectScatterResources,
        animation: &AnimatedLightBuffers,
        delta: Option<&AnimatedBillboardDirectScatterDeltaVolumesSection>,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        grid_dimensions: [u32; 3],
    ) -> Self {
        if !scatter.has_animated_deltas {
            return Self { pipeline: None };
        }
        let Some(delta) = delta else {
            return Self { pipeline: None };
        };
        let Some(output) = scatter.composed_storage_view.as_ref() else {
            return Self { pipeline: None };
        };

        let affinity_dimensions = delta.affinity_dims;
        debug_assert_eq!(delta.affinity_factor, 4);
        debug_assert_eq!(
            affinity_dimensions,
            grid_dimensions.map(|dimension| dimension.div_ceil(delta.affinity_factor as u32)),
            "section-48 CSR must cover the same affinity grid as section 45",
        );
        let grid_bytes = scatter_grid_bytes(grid_dimensions, affinity_dimensions);
        let delta_bytes = pad_storage_bytes(u16_slice_to_bytes(&delta.delta_rgba), 4);
        let offset_bytes = pad_storage_bytes(u32_slice_to_bytes(&delta.affinity_offsets), 8);
        let light_bytes = pad_storage_bytes(u32_slice_to_bytes(&delta.affinity_lights), 4);
        let descriptor_index_bytes =
            pad_storage_bytes(u32_slice_to_bytes(&delta.animation_descriptor_indices), 4);

        let grid_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Billboard Direct Scatter Compose Grid"),
            contents: &grid_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let delta_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Billboard Direct Scatter Compose Deltas (f16)"),
            contents: &delta_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let offset_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Billboard Direct Scatter Compose CSR Offsets"),
            contents: &offset_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let light_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Billboard Direct Scatter Compose CSR Lights"),
            contents: &light_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let descriptor_index_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Billboard Direct Scatter Compose Descriptor Indices"),
                contents: &descriptor_index_bytes,
                usage: wgpu::BufferUsages::STORAGE,
            });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Billboard Direct Scatter Compose BGL"),
            entries: &compose_bgl_entries(),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Billboard Direct Scatter Compose Pipeline Layout"),
            bind_group_layouts: &[Some(uniform_bind_group_layout), Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let shader_source = concat!(
            include_str!("../shaders/billboard_direct_scatter_compose.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Billboard Direct Scatter Compose Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Billboard Direct Scatter Compose Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("compose_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Billboard Direct Scatter Compose Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: BIND_BASE,
                    resource: wgpu::BindingResource::TextureView(&scatter.base_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_OUTPUT,
                    resource: wgpu::BindingResource::TextureView(output),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_GRID,
                    resource: grid_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_DELTAS,
                    resource: delta_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_OFFSETS,
                    resource: offset_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_DESCRIPTORS,
                    resource: animation.descriptors.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_SAMPLES,
                    resource: animation.anim_samples.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_AFFINITY_LIGHTS,
                    resource: light_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_DESCRIPTOR_INDICES,
                    resource: descriptor_index_buffer.as_entire_binding(),
                },
            ],
        });

        Self {
            pipeline: Some(ComposePipeline {
                pipeline,
                bind_group,
                dispatch_dimensions: affinity_dimensions,
                pending_copy_through: true,
                was_active: false,
                last_composed_mask: LightTermMask::ALL,
            }),
        }
    }

    pub(super) fn dispatch_if_needed(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        active: bool,
        light_term_mask: LightTermMask,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        let Some(pipeline) = self.pipeline.as_mut() else {
            return;
        };
        if !scatter_compose_should_dispatch(
            active,
            pipeline.pending_copy_through,
            pipeline.was_active,
            light_term_mask,
            pipeline.last_composed_mask,
        ) {
            return;
        }

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Billboard Direct Scatter Compose"),
            timestamp_writes,
        });
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, uniform_bind_group, &[]);
        pass.set_bind_group(1, &pipeline.bind_group, &[]);
        pass.dispatch_workgroups(
            pipeline.dispatch_dimensions[0].max(1),
            pipeline.dispatch_dimensions[1].max(1),
            pipeline.dispatch_dimensions[2].max(1),
        );
        drop(pass);

        pipeline.pending_copy_through = false;
        pipeline.was_active = active;
        pipeline.last_composed_mask = light_term_mask;
    }
}

fn scatter_compose_should_dispatch(
    active: bool,
    pending_copy_through: bool,
    was_active: bool,
    frame_light_term_mask: LightTermMask,
    last_composed_mask: LightTermMask,
) -> bool {
    active || pending_copy_through || was_active || frame_light_term_mask != last_composed_mask
}

/// WGSL `ScatterGrid`: grid dimensions followed by one padding u32, then the
/// derived 4×4×4 affinity dimensions. Kept local because section 48 stores
/// dense samples rather than the octahedral compose grid contract.
fn scatter_grid_bytes(grid_dimensions: [u32; 3], affinity_dimensions: [u32; 3]) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    for (index, value) in grid_dimensions.into_iter().enumerate() {
        let start = index * 4;
        bytes[start..start + 4].copy_from_slice(&value.to_ne_bytes());
    }
    for (index, value) in affinity_dimensions.into_iter().enumerate() {
        let start = 16 + index * 4;
        bytes[start..start + 4].copy_from_slice(&value.to_ne_bytes());
    }
    bytes
}

fn compose_bgl_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        texture_entry(BIND_BASE),
        storage_texture_entry(BIND_OUTPUT),
        uniform_entry(BIND_GRID),
        storage_entry(BIND_DELTAS),
        storage_entry(BIND_OFFSETS),
        storage_entry(BIND_DESCRIPTORS),
        storage_entry(BIND_SAMPLES),
        storage_entry(BIND_AFFINITY_LIGHTS),
        storage_entry(BIND_DESCRIPTOR_INDICES),
    ]
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D3,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: wgpu::TextureFormat::Rgba16Float,
            view_dimension: wgpu::TextureViewDimension::D3,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_render_cpu::sh_compose::{AnimatedLightScaleDescriptor, animated_light_scale};

    fn assert_rgb_close(actual: [f32; 3], expected: [f32; 3]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!(
                (actual - expected).abs() <= 1.0e-6,
                "expected {expected}, got {actual}"
            );
        }
    }

    fn compose_scatter_reference(
        base: [f32; 3],
        mask: LightTermMask,
        affinity_lights: &[u32],
        descriptor_indices: &[u32],
        descriptor_scales: &[[f32; 3]],
        deltas: &[[f32; 3]],
    ) -> [f32; 3] {
        let mut accum = if mask.contains(LightTermMask::BAKED_DIRECT_STATIC) {
            base
        } else {
            [0.0; 3]
        };
        if !mask.contains(LightTermMask::BAKED_DIRECT_ANIMATED) {
            return accum;
        }
        for (&light, &delta) in affinity_lights.iter().zip(deltas) {
            let Some(&descriptor_index) = descriptor_indices.get(light as usize) else {
                continue;
            };
            let Some(&scale) = descriptor_scales.get(descriptor_index as usize) else {
                continue;
            };
            for channel in 0..3 {
                accum[channel] += scale[channel] * delta[channel];
            }
        }
        accum
    }

    #[test]
    fn scatter_compose_schedules_initial_copy_active_and_one_settle_frame() {
        assert!(scatter_compose_should_dispatch(
            false,
            true,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(scatter_compose_should_dispatch(
            true,
            false,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(scatter_compose_should_dispatch(
            false,
            false,
            true,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(!scatter_compose_should_dispatch(
            false,
            false,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
    }

    #[test]
    fn scatter_compose_reacts_to_any_mask_bit_change() {
        let mut changed = LightTermMask::ALL;
        changed.set_enabled(LightTermMask::SPECULAR, false);
        assert!(scatter_compose_should_dispatch(
            false,
            false,
            false,
            changed,
            LightTermMask::ALL,
        ));

        // P11 regression: clearing static direct must schedule the compose that
        // removes the base before a composed texture is sampled via animated.
        let mut static_disabled = LightTermMask::ALL;
        static_disabled.set_enabled(LightTermMask::BAKED_DIRECT_STATIC, false);
        assert!(scatter_compose_should_dispatch(
            false,
            false,
            false,
            static_disabled,
            LightTermMask::ALL,
        ));
    }

    #[test]
    fn scatter_compose_uses_active_flags_even_when_a_curve_evaluates_to_zero() {
        let zero_scale = animated_light_scale(
            Some(AnimatedLightScaleDescriptor {
                period: 1.0,
                phase: 0.0,
                base_color: [1.0; 3],
                brightness: &[0.0],
                color: &[],
                is_active: true,
            }),
            0.0,
        );
        assert_rgb_close(zero_scale, [0.0; 3]);
        assert!(scatter_compose_should_dispatch(
            true,
            false,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
    }

    #[test]
    fn scatter_reference_is_base_plus_descriptor_indexed_scaled_deltas() {
        let scale = animated_light_scale(
            Some(AnimatedLightScaleDescriptor {
                period: 1.0,
                phase: 0.0,
                base_color: [2.0; 3],
                brightness: &[0.5],
                color: &[[0.5, 1.0, 0.25]],
                is_active: true,
            }),
            0.0,
        );
        assert_rgb_close(scale, [0.5, 1.0, 0.25]);

        // Affinity light 1 intentionally resolves through descriptor index 1;
        // index 0 is a sentinel-like wrong scale, proving the section-48 map
        // is consumed rather than using affinity light ids as descriptor ids.
        let composed = compose_scatter_reference(
            [1.0, 2.0, 3.0],
            LightTermMask::ALL,
            &[1],
            &[0, 1],
            &[[99.0; 3], scale],
            &[[2.0, 3.0, 4.0]],
        );
        assert_rgb_close(composed, [2.0, 5.0, 4.0]);

        let mut animated_disabled = LightTermMask::ALL;
        animated_disabled.set_enabled(LightTermMask::BAKED_DIRECT_ANIMATED, false);
        assert_rgb_close(
            compose_scatter_reference(
                [1.0, 2.0, 3.0],
                animated_disabled,
                &[1],
                &[0, 1],
                &[[99.0; 3], scale],
                &[[2.0, 3.0, 4.0]],
            ),
            [1.0, 2.0, 3.0],
        );

        let mut static_disabled = LightTermMask::ALL;
        static_disabled.set_enabled(LightTermMask::BAKED_DIRECT_STATIC, false);
        assert_rgb_close(
            compose_scatter_reference(
                [1.0, 2.0, 3.0],
                static_disabled,
                &[1],
                &[0, 1],
                &[[99.0; 3], scale],
                &[[2.0, 3.0, 4.0]],
            ),
            [1.0, 3.0, 1.0],
        );
    }

    #[test]
    fn scatter_grid_preserves_x_fastest_4_probe_affinity_lattice() {
        let bytes = scatter_grid_bytes([5, 4, 9], [2, 1, 3]);
        assert_eq!(u32::from_ne_bytes(bytes[0..4].try_into().unwrap()), 5);
        assert_eq!(u32::from_ne_bytes(bytes[16..20].try_into().unwrap()), 2);
        assert_eq!(u32::from_ne_bytes(bytes[24..28].try_into().unwrap()), 3);
    }

    #[test]
    fn compose_buffers_are_compute_only() {
        for entry in compose_bgl_entries() {
            assert_eq!(entry.visibility, wgpu::ShaderStages::COMPUTE);
        }
    }

    #[test]
    fn compose_shader_copies_base_before_accumulating_csr_deltas() {
        let source = include_str!("../shaders/billboard_direct_scatter_compose.wgsl");
        let base_copy = source
            .find("var accum = textureLoad(base_scatter")
            .expect("compose must seed each probe from the section-47 base");
        let csr_loop = source
            .find("for (var entry = start; entry < end")
            .expect("compose must accumulate section-48 CSR deltas");
        assert!(base_copy < csr_loop);
    }

    #[test]
    fn scatter_compose_shader_parses_and_validates() {
        let source = concat!(
            include_str!("../shaders/billboard_direct_scatter_compose.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
        );
        let module = naga::front::wgsl::parse_str(source)
            .expect("billboard direct-scatter compose shader should parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("billboard direct-scatter compose shader should validate");
        assert!(module.entry_points.iter().any(|entry| {
            entry.name == "compose_main" && entry.stage == naga::ShaderStage::Compute
        }));
    }
}
