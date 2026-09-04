// Animated-direct SH addition pass. See `context/lib/rendering_pipeline.md` §4,
// “Animated direct SH for dynamic receivers”; separate from promotion composition so section-45 resources and bindings stay isolated from legacy Case 1.

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
#[cfg(feature = "dev-tools")]
use postretro_render_cpu::sh_compose::ComposeStorageFootprint;
use postretro_render_cpu::sh_compose::{
    ComposeGridParams, build_animated_direct_delta_buffers, build_compose_grid_bytes,
    pad_storage_bytes, u16_slice_to_bytes, u32_slice_to_bytes,
};

use super::direct_sh_compose::{
    BIND_AFFINITY_LIGHTS, BIND_AFFINITY_OFFSETS, BIND_ANIMATION_DESCRIPTOR_INDICES,
    BIND_ANIMATION_DESCRIPTORS, BIND_ANIMATION_SAMPLES, BIND_BASE_SAMPLER, BIND_DELTA_SUBBLOCKS,
    nearest_sampler, sampler_bgl_entry, storage_bgl_entry, storage_texture_bgl_entry,
    texture_bgl_entry, uniform_bgl_entry,
};
use super::direct_sh_resources::DirectAtlasLayout;
use super::sh_indirection::{WGSL_DECODE_HELPER, probe_indirection_storage_bytes};
use super::sh_volume::AnimatedLightBuffers;

/// Pass-B-only dev-tools override. Its `light_index` is in the
/// `AnimatedBakedLights` namespace used by section 45, not the promotion
/// selection namespace consumed by Pass A.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AnimatedDirectShDebugOverride {
    pub enabled: bool,
    pub light_index: u32,
    pub weight: f32,
}

impl Default for AnimatedDirectShDebugOverride {
    fn default() -> Self {
        Self {
            enabled: false,
            light_index: 0,
            weight: 0.0,
        }
    }
}

impl AnimatedDirectShDebugOverride {
    pub fn active(self) -> bool {
        self.enabled && self.weight > 0.0
    }

    pub(super) fn bytes(self) -> [u8; ANIMATED_DEBUG_OVERRIDE_SIZE] {
        let mut bytes = [0u8; ANIMATED_DEBUG_OVERRIDE_SIZE];
        bytes[0..4].copy_from_slice(&(self.enabled as u32).to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.light_index.to_ne_bytes());
        bytes[16..20].copy_from_slice(&self.weight.clamp(0.0, 1.0).to_ne_bytes());
        bytes
    }
}

const BIND_ANIMATED_DEBUG_OVERRIDE: u32 = 26;
/// Combined low/high valid-probe mask words, one coarsening level per cell,
/// then one f16-half payload offset per post-drop CSR entry. Binding 26 is the
/// pass-B debug override.
const BIND_DELTA_COMPACTION_META: u32 = 27;
/// Load-derived id-34 indirection words. Kept at an otherwise unused binding
/// until Task 4 switches this pass from dense grid writes to stored slots.
const BIND_PROBE_INDIRECTION: u32 = 28;
const ANIMATED_DEBUG_OVERRIDE_SIZE: usize = 32;
#[cfg(feature = "dev-tools")]
const ANIMATED_DIRECT_FOOTPRINT_LABEL: &str = "DIRECT SH compose id-45 animated-add @group(1)";

pub(super) struct AnimatedDirectShComposePipeline {
    pub(super) pipeline: wgpu::ComputePipeline,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) debug_override_buffer: wgpu::Buffer,
    pub(super) last_debug_override_bytes: [u8; ANIMATED_DEBUG_OVERRIDE_SIZE],
}

pub(super) fn build_animated_direct_pass(
    device: &wgpu::Device,
    animation: &AnimatedLightBuffers,
    layout: DirectAtlasLayout,
    probe_indirection_words: &[u32],
    animated_delta: &AnimatedDirectShDeltaVolumesSection,
    intermediate_sampled_view: &wgpu::TextureView,
    output_storage_view: &wgpu::TextureView,
    uniform_bind_group_layout: &wgpu::BindGroupLayout,
) -> AnimatedDirectShComposePipeline {
    let buffers = build_animated_direct_delta_buffers(Some(animated_delta), layout.grid_dimensions);
    let subblock_bytes = pad_storage_bytes(u16_slice_to_bytes(&buffers.delta_subblocks), 4);
    let compaction_meta_bytes =
        pad_storage_bytes(u32_slice_to_bytes(&buffers.compaction_meta_words()), 4);
    let offsets_bytes = pad_storage_bytes(u32_slice_to_bytes(&buffers.affinity_offsets), 8);
    let lights_bytes = pad_storage_bytes(u32_slice_to_bytes(&buffers.affinity_lights), 4);
    let descriptor_indices_bytes =
        pad_storage_bytes(u32_slice_to_bytes(&buffers.animation_descriptor_indices), 4);

    #[cfg(feature = "dev-tools")]
    ComposeStorageFootprint {
        delta_subblocks_bytes: subblock_bytes.len(),
        delta_compaction_meta_bytes: compaction_meta_bytes.len(),
        affinity_offsets_bytes: offsets_bytes.len(),
        affinity_lights_bytes: lights_bytes.len(),
        animation_descriptor_indices_bytes: descriptor_indices_bytes.len(),
    }
    .log(ANIMATED_DIRECT_FOOTPRINT_LABEL);

    use wgpu::util::DeviceExt;
    let delta_subblocks_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Animated Direct SH Compose Delta Subblocks (f16)"),
        contents: &subblock_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let delta_compaction_meta_buffer =
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Animated Direct SH Compose Delta Compaction Meta"),
            contents: &compaction_meta_bytes,
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
    let probe_indirection_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Animated Direct SH Compose Probe Indirection"),
        contents: &probe_indirection_storage_bytes(probe_indirection_words),
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
            // Retain the existing 64-byte uniform layout: its former compact
            // tail now repeats the stored atlas geometry.
            compact_atlas_tiles_per_row: layout.atlas_tiles_per_row,
            compact_atlas_tiles_per_layer: layout.tiles_per_layer,
        }),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let debug_override_bytes = AnimatedDirectShDebugOverride::default().bytes();
    let debug_override_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Animated Direct SH Compose Debug Override"),
        contents: &debug_override_bytes,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
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
    let shader_source = [
        include_str!("../shaders/animated_direct_sh_compose.wgsl"),
        "\n",
        include_str!("../shaders/curve_eval.wgsl"),
        "\n",
        WGSL_DECODE_HELPER,
    ]
    .concat();
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
                binding: BIND_DELTA_COMPACTION_META,
                resource: delta_compaction_meta_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_AFFINITY_OFFSETS,
                resource: affinity_offsets_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_ANIMATION_DESCRIPTORS,
                resource: animation.descriptors.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_ANIMATION_SAMPLES,
                resource: animation.anim_samples.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_AFFINITY_LIGHTS,
                resource: affinity_lights_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_ANIMATION_DESCRIPTOR_INDICES,
                resource: descriptor_indices_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_ANIMATED_DEBUG_OVERRIDE,
                resource: debug_override_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_PROBE_INDIRECTION,
                resource: probe_indirection_buffer.as_entire_binding(),
            },
        ],
    });

    AnimatedDirectShComposePipeline {
        pipeline,
        bind_group,
        debug_override_buffer,
        last_debug_override_bytes: debug_override_bytes,
    }
}

fn animated_compose_bgl_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        texture_bgl_entry(0),
        sampler_bgl_entry(BIND_BASE_SAMPLER),
        storage_texture_bgl_entry(1),
        uniform_bgl_entry(18),
        storage_bgl_entry(BIND_DELTA_SUBBLOCKS),
        storage_bgl_entry(BIND_DELTA_COMPACTION_META),
        storage_bgl_entry(BIND_AFFINITY_OFFSETS),
        storage_bgl_entry(BIND_ANIMATION_DESCRIPTORS),
        storage_bgl_entry(BIND_ANIMATION_SAMPLES),
        storage_bgl_entry(BIND_AFFINITY_LIGHTS),
        storage_bgl_entry(BIND_ANIMATION_DESCRIPTOR_INDICES),
        uniform_bgl_entry(BIND_ANIMATED_DEBUG_OVERRIDE),
        storage_bgl_entry(BIND_PROBE_INDIRECTION),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animated_pass_binds_compaction_metadata_and_indirection_within_eight_storage_buffers() {
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
                BIND_DELTA_COMPACTION_META,
                BIND_AFFINITY_OFFSETS,
                BIND_ANIMATION_DESCRIPTORS,
                BIND_ANIMATION_SAMPLES,
                BIND_AFFINITY_LIGHTS,
                BIND_ANIMATION_DESCRIPTOR_INDICES,
                BIND_PROBE_INDIRECTION,
            ]
        );
        assert_eq!(storage_bindings.len(), 8);
        assert!(animated_compose_bgl_entries().into_iter().any(|entry| {
            entry.binding == BIND_ANIMATED_DEBUG_OVERRIDE
                && matches!(
                    entry.ty,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        ..
                    }
                )
        }));
        assert!(animated_compose_bgl_entries().into_iter().any(|entry| {
            entry.binding == BIND_DELTA_COMPACTION_META
                && matches!(
                    entry.ty,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        ..
                    }
                )
        }));
        assert_eq!(BIND_DELTA_COMPACTION_META, 27);
    }

    #[test]
    fn animated_shader_uses_indirection_to_gate_stored_slot_reconstruction() {
        let source = include_str!("../shaders/animated_direct_sh_compose.wgsl");
        assert!(
            source.contains("let output_is_stored = stored_slot.write;")
                && source.contains("@group(1) @binding(28) var<storage, read> probe_indirection"),
            "Pass B must derive stored-slot writes from Task 3's id-34 indirection"
        );
        assert!(
            !source.contains("enable f16"),
            "animated-direct delta payloads remain Rgba16Float read through f32 unpacking"
        );
    }

    #[test]
    fn animated_shader_reads_group_zero_mask_before_adding_delta() {
        let source = include_str!("../shaders/animated_direct_sh_compose.wgsl");
        assert!(source.contains("light_term_mask: u32,"));
        assert!(source.contains("const LIGHT_TERM_BAKED_DIRECT_ANIMATED: u32 = 0x10u;"));
        assert!(
            source.contains(
                "if ((uniforms.light_term_mask & LIGHT_TERM_BAKED_DIRECT_ANIMATED) == 0u)"
            )
        );
    }

    #[test]
    fn animated_coarsened_compose_uses_one_brick_workgroup_and_kept_shared_tiles() {
        let source = include_str!("../shaders/animated_direct_sh_compose.wgsl");

        assert!(source.contains("@builtin(workgroup_id) brick"));
        assert!(source.contains("var<workgroup> shared_kept_tiles"));
        assert!(source.contains(
            "return grid.affinity_dims.x * grid.affinity_dims.y * grid.affinity_dims.z * 3u"
        ));
        assert!(source.contains("countOneBits(kept_probe_mask_word"));
        assert!(source.contains("if (level == 0u)"));
        assert!(source.contains("if (level == 1u && local_probe_is_kept"));
        assert!(source.contains("if (level == 2u)"));
        assert!(
            source.contains("fn slot_tile_origin(slot: u32)")
                && !source.contains("fn atlas_tile_origin(")
                && source.contains("brick_indirection.level == 1u && local_probe_is_l1_corner")
                && source.contains("brick_indirection.level == 2u && local_probe == 0u")
                && source.contains("select(0.0, 1.0, stored_slot.valid)"),
            "Pass B must sample the compact intermediate and write the same stored slots"
        );
    }

    #[test]
    fn animated_debug_override_bytes_encode_animated_baked_light_index() {
        let bytes = AnimatedDirectShDebugOverride {
            enabled: true,
            light_index: 13,
            weight: 0.5,
        }
        .bytes();

        assert_eq!(bytes.len(), ANIMATED_DEBUG_OVERRIDE_SIZE);
        assert_eq!(u32::from_ne_bytes(bytes[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_ne_bytes(bytes[4..8].try_into().unwrap()), 13);
        assert!((f32::from_ne_bytes(bytes[16..20].try_into().unwrap()) - 0.5).abs() < f32::EPSILON);
        assert!(bytes[8..16].iter().all(|&byte| byte == 0));
        assert!(bytes[20..32].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn shader_parses_and_exports_pass_b() {
        let source = [
            include_str!("../shaders/animated_direct_sh_compose.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
            "\n",
            super::WGSL_DECODE_HELPER,
        ]
        .concat();
        let module = naga::front::wgsl::parse_str(&source)
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
