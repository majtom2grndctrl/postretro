// Direct SH compose compute pass: subtracts selected static-light direct SH
// deltas from the baked direct atlas before entity/billboard sampling.
// See: context/lib/rendering_pipeline.md §7.1

use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_render_cpu::sh_compose::{
    ComposeGridParams, build_compose_grid_bytes, build_direct_delta_buffers, pad_storage_bytes,
    u16_slice_to_bytes, u32_slice_to_bytes,
};

use super::sh_volume::ShVolumeResources;

const BIND_BASE_SAMPLER: u32 = 2;
const BIND_DELTA_SUBBLOCKS: u32 = 20;
const BIND_AFFINITY_OFFSETS: u32 = 21;
const BIND_AFFINITY_LIGHTS: u32 = 24;
const BIND_SELECTION_WEIGHTS: u32 = 26;
const BIND_DEBUG_OVERRIDE: u32 = 27;
const DEBUG_OVERRIDE_SIZE: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DirectShDebugOverride {
    pub enabled: bool,
    pub selection_index: u32,
    pub weight: f32,
}

impl Default for DirectShDebugOverride {
    fn default() -> Self {
        Self {
            enabled: false,
            selection_index: 0,
            weight: 0.0,
        }
    }
}

impl DirectShDebugOverride {
    pub fn active(self) -> bool {
        self.enabled && self.weight > 0.0
    }
}

struct DirectShComposePipeline {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    dispatch_dimensions: [u32; 3],
    debug_override_buffer: wgpu::Buffer,
    pending_copy_through: bool,
    was_active: bool,
    last_debug_override_bytes: [u8; DEBUG_OVERRIDE_SIZE],
}

pub(crate) struct DirectShComposeResources {
    pipeline: Option<DirectShComposePipeline>,
}

impl DirectShComposeResources {
    pub fn disabled() -> Self {
        Self { pipeline: None }
    }

    pub fn new(
        device: &wgpu::Device,
        sh: &ShVolumeResources,
        direct_section: Option<&DirectShVolumeSection>,
        delta: Option<&DirectShDeltaVolumesSection>,
        weights_buffer: &wgpu::Buffer,
    ) -> Self {
        let Some(section) = direct_section else {
            return Self::disabled();
        };
        let Some(delta) = delta.filter(|d| !d.affinity_lights.is_empty()) else {
            return Self::disabled();
        };
        let Some(composed_storage_view) = sh.direct_composed_storage_view.as_ref() else {
            return Self::disabled();
        };

        let buffers = build_direct_delta_buffers(Some(delta), section.grid_dimensions);
        let subblock_bytes = pad_storage_bytes(u16_slice_to_bytes(&buffers.delta_subblocks), 4);
        let offsets_bytes = pad_storage_bytes(u32_slice_to_bytes(&buffers.affinity_offsets), 8);
        let lights_bytes = pad_storage_bytes(u32_slice_to_bytes(&buffers.affinity_lights), 4);

        use wgpu::util::DeviceExt;
        let delta_subblocks_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Direct SH Compose Delta Subblocks (f16)"),
            contents: &subblock_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let affinity_offsets_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Direct SH Compose Affinity Offsets"),
                contents: &offsets_bytes,
                usage: wgpu::BufferUsages::STORAGE,
            });
        let affinity_lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Direct SH Compose Affinity Lights"),
            contents: &lights_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });

        let grid_bytes = build_compose_grid_bytes(ComposeGridParams {
            grid_dimensions: section.grid_dimensions,
            atlas_dimensions: section.atlas_dimensions,
            tile_dimension: section.tile_dimension,
            tile_border: section.tile_border,
            atlas_tiles_per_row: section.atlas_tiles_per_row,
            tiles_per_layer: section.tiles_per_layer,
            atlas_layer_count: section.layer_count,
            affinity_dims: buffers.affinity_dims,
        });
        let grid_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Direct SH Compose Grid Dims"),
            contents: &grid_bytes[..],
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let debug_override_bytes = debug_override_bytes(DirectShDebugOverride::default());
        let debug_override_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Direct SH Compose Debug Override"),
            contents: &debug_override_bytes,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Direct SH Compose BGL"),
            entries: &compose_bgl_entries(),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Direct SH Compose Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Direct SH Compose Shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../shaders/direct_sh_compose.wgsl").into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Direct SH Compose Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("compose_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        // Non-filtering (nearest) sampler for the BC6H base atlas point-fetch.
        // Nearest at a texel-center UV returns the decoded texel verbatim — the
        // compose subtraction must not blend across probe/tile texel boundaries.
        let base_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Direct SH Compose Base Atlas Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Direct SH Compose Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&sh.direct_base_atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_BASE_SAMPLER,
                    resource: wgpu::BindingResource::Sampler(&base_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(composed_storage_view),
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
                    binding: BIND_AFFINITY_LIGHTS,
                    resource: affinity_lights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_SELECTION_WEIGHTS,
                    resource: weights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_DEBUG_OVERRIDE,
                    resource: debug_override_buffer.as_entire_binding(),
                },
            ],
        });

        log::info!(
            "[Renderer] Direct SH compose: {} selected-light CSR entr(y/ies), atlas {}×{}",
            delta.affinity_lights.len(),
            section.atlas_dimensions[0],
            section.atlas_dimensions[1],
        );

        Self {
            pipeline: Some(DirectShComposePipeline {
                pipeline,
                bind_group,
                dispatch_dimensions: [
                    section.atlas_dimensions[0],
                    section.atlas_dimensions[1],
                    section.layer_count,
                ],
                debug_override_buffer,
                pending_copy_through: true,
                was_active: false,
                last_debug_override_bytes: debug_override_bytes,
            }),
        }
    }

    pub fn dispatch_if_needed(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        active: bool,
        debug_override: DirectShDebugOverride,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        let Some(pipeline) = self.pipeline.as_mut() else {
            return;
        };

        let debug_bytes = debug_override_bytes(debug_override);
        if debug_bytes != pipeline.last_debug_override_bytes {
            queue.write_buffer(&pipeline.debug_override_buffer, 0, &debug_bytes);
            pipeline.last_debug_override_bytes = debug_bytes;
        }

        let should_dispatch = direct_compose_should_dispatch(
            active,
            pipeline.pending_copy_through,
            pipeline.was_active,
        );
        if !should_dispatch {
            return;
        }

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Direct SH Compose"),
            timestamp_writes,
        });
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &pipeline.bind_group, &[]);
        pass.dispatch_workgroups(
            pipeline.dispatch_dimensions[0].div_ceil(8).max(1),
            pipeline.dispatch_dimensions[1].div_ceil(8).max(1),
            pipeline.dispatch_dimensions[2].max(1),
        );
        pipeline.pending_copy_through = false;
        pipeline.was_active = active;
    }
}

fn compose_bgl_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        // Non-filtering sampler for the BC6H base atlas point-fetch. The base
        // texture stays `Float { filterable: true }`; a `NonFiltering` sampler is
        // valid against a filterable texture (it simply never filters), so the
        // nearest fetch reads the exact decoded texel.
        wgpu::BindGroupLayoutEntry {
            binding: BIND_BASE_SAMPLER,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
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
        wgpu::BindGroupLayoutEntry {
            binding: BIND_SELECTION_WEIGHTS,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: BIND_DEBUG_OVERRIDE,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
    ]
}

fn debug_override_bytes(value: DirectShDebugOverride) -> [u8; DEBUG_OVERRIDE_SIZE] {
    let mut bytes = [0u8; DEBUG_OVERRIDE_SIZE];
    bytes[0..4].copy_from_slice(&(value.enabled as u32).to_ne_bytes());
    bytes[4..8].copy_from_slice(&value.selection_index.to_ne_bytes());
    bytes[16..20].copy_from_slice(&value.weight.clamp(0.0, 1.0).to_ne_bytes());
    bytes
}

fn direct_compose_should_dispatch(
    active: bool,
    pending_copy_through: bool,
    was_active: bool,
) -> bool {
    active || pending_copy_through || was_active
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_compose_schedules_load_active_and_zero_transition() {
        assert!(direct_compose_should_dispatch(false, true, false));
        assert!(direct_compose_should_dispatch(true, false, false));
        assert!(direct_compose_should_dispatch(false, false, true));
        assert!(!direct_compose_should_dispatch(false, false, false));
    }

    #[test]
    fn debug_override_bytes_encodes_fields_at_correct_offsets() {
        let bytes = debug_override_bytes(DirectShDebugOverride {
            enabled: true,
            selection_index: 7,
            weight: 0.5,
        });
        assert_eq!(bytes.len(), DEBUG_OVERRIDE_SIZE);
        assert_eq!(u32::from_ne_bytes(bytes[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_ne_bytes(bytes[4..8].try_into().unwrap()), 7);
        assert_eq!(f32::from_ne_bytes(bytes[16..20].try_into().unwrap()), 0.5);
        // Padding fields (8..16, 20..32) carry no data; confirm they stay zeroed.
        assert!(bytes[8..16].iter().all(|&b| b == 0));
        assert!(bytes[20..32].iter().all(|&b| b == 0));
    }

    #[test]
    fn direct_sh_compose_shader_parses_and_exports_compose_main() {
        let module =
            naga::front::wgsl::parse_str(include_str!("../shaders/direct_sh_compose.wgsl"))
                .expect("direct_sh_compose.wgsl should parse as WGSL");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("direct_sh_compose.wgsl should validate");
        let has_compose = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "compose_main" && ep.stage == naga::ShaderStage::Compute);
        assert!(has_compose, "compose_main entry point missing");
    }
}
