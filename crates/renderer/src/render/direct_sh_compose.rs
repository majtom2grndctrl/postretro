// Direct SH compose passes: promotion subtraction plus optional animated-direct addition.
// See: context/lib/rendering_pipeline.md §7.1

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
#[cfg(feature = "dev-tools")]
use postretro_render_cpu::sh_compose::ComposeStorageFootprint;
use postretro_render_cpu::sh_compose::{
    ComposeGridParams, DirectDeltaComposeBuffers, build_compose_grid_bytes,
    build_direct_delta_buffers, pad_storage_bytes, u16_slice_to_bytes, u32_slice_to_bytes,
};

use super::animated_direct_sh_compose::{
    AnimatedDirectShComposePipeline, AnimatedDirectShDebugOverride, build_animated_direct_pass,
};
use super::sh_volume::ShVolumeResources;

pub(super) const BIND_BASE_SAMPLER: u32 = 2;
pub(super) const BIND_DELTA_SUBBLOCKS: u32 = 20;
pub(super) const BIND_AFFINITY_OFFSETS: u32 = 21;
pub(super) const BIND_ANIMATION_DESCRIPTORS: u32 = 22;
pub(super) const BIND_ANIMATION_SAMPLES: u32 = 23;
pub(super) const BIND_AFFINITY_LIGHTS: u32 = 24;
pub(super) const BIND_ANIMATION_DESCRIPTOR_INDICES: u32 = 25;
const BIND_SELECTION_WEIGHTS: u32 = 26;
const BIND_DEBUG_OVERRIDE: u32 = 27;
const DEBUG_OVERRIDE_SIZE: usize = 32;
#[cfg(feature = "dev-tools")]
const DIRECT_PROMOTION_FOOTPRINT_LABEL: &str = "DIRECT SH compose id-41 promotion @group(0)";

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

pub(super) struct DirectShComposeDebugOverrides {
    pub(super) promotion: DirectShDebugOverride,
    pub(super) animated: AnimatedDirectShDebugOverride,
}

pub(super) struct DirectShComposeTimestampWrites<'a> {
    pub(super) promotion: Option<wgpu::ComputePassTimestampWrites<'a>>,
    pub(super) animated: Option<wgpu::ComputePassTimestampWrites<'a>>,
}

struct DirectShComposePipeline {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    dispatch_dimensions: [u32; 3],
    debug_override_buffer: wgpu::Buffer,
    pending_copy_through: bool,
    was_active: bool,
    last_debug_override_bytes: [u8; DEBUG_OVERRIDE_SIZE],
    /// Present only for section-45 maps. Case 1 retains exactly one pass and
    /// writes the final sampled atlas directly.
    animated_add: Option<AnimatedDirectShComposePipeline>,
}

#[derive(Clone, Copy)]
pub(super) struct DirectComposeLayout {
    pub(super) grid_dimensions: [u32; 3],
    pub(super) atlas_dimensions: [u32; 2],
    pub(super) tile_dimension: u32,
    pub(super) tile_border: u32,
    pub(super) atlas_tiles_per_row: u32,
    pub(super) tiles_per_layer: u32,
    pub(super) atlas_layer_count: u32,
}

struct DirectPromotionStorage {
    buffers: DirectDeltaComposeBuffers,
    subblock_bytes: Vec<u8>,
    offsets_bytes: Vec<u8>,
    lights_bytes: Vec<u8>,
}

impl DirectPromotionStorage {
    fn new(delta: Option<&DirectShDeltaVolumesSection>, grid_dimensions: [u32; 3]) -> Self {
        let buffers = build_direct_delta_buffers(delta, grid_dimensions);
        let subblock_bytes = pad_storage_bytes(u16_slice_to_bytes(&buffers.delta_subblocks), 4);
        let offsets_bytes = pad_storage_bytes(u32_slice_to_bytes(&buffers.affinity_offsets), 8);
        let lights_bytes = pad_storage_bytes(u32_slice_to_bytes(&buffers.affinity_lights), 4);
        Self {
            buffers,
            subblock_bytes,
            offsets_bytes,
            lights_bytes,
        }
    }

    #[cfg(feature = "dev-tools")]
    fn footprint(&self) -> ComposeStorageFootprint {
        ComposeStorageFootprint {
            delta_subblocks_bytes: self.subblock_bytes.len(),
            affinity_offsets_bytes: self.offsets_bytes.len(),
            affinity_lights_bytes: self.lights_bytes.len(),
            // The id-41 promotion pass has no descriptor-index binding. Case
            // 2's id-45 animated-add storage belongs to its sibling pass.
            animation_descriptor_indices_bytes: 0,
        }
    }

    #[cfg(feature = "dev-tools")]
    fn log_footprint(&self) {
        self.footprint().log(DIRECT_PROMOTION_FOOTPRINT_LABEL);
    }
}

pub(crate) struct DirectShComposeResources {
    pipeline: Option<DirectShComposePipeline>,
}

impl DirectShComposeResources {
    pub fn disabled() -> Self {
        Self { pipeline: None }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        sh: &ShVolumeResources,
        direct_section: Option<&DirectShVolumeSection>,
        delta: Option<&DirectShDeltaVolumesSection>,
        animated_delta: Option<&AnimatedDirectShDeltaVolumesSection>,
        weights_buffer: &wgpu::Buffer,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        match animated_delta {
            // Case 2 is selected only at level load from section presence.
            Some(animated_delta) => Self::new_case2(
                device,
                sh,
                direct_section,
                delta,
                animated_delta,
                weights_buffer,
                uniform_bind_group_layout,
            ),
            // Preserve the promotion-only construction path, binding set, and
            // single output target exactly when section 45 is absent.
            None => Self::new_case1(device, sh, direct_section, delta, weights_buffer),
        }
    }

    fn new_case1(
        device: &wgpu::Device,
        sh: &ShVolumeResources,
        direct_section: Option<&DirectShVolumeSection>,
        delta: Option<&DirectShDeltaVolumesSection>,
        weights_buffer: &wgpu::Buffer,
    ) -> Self {
        let Some(section) = direct_section else {
            return Self::disabled();
        };
        let Some(delta) = delta.filter(|delta| !delta.affinity_lights.is_empty()) else {
            return Self::disabled();
        };
        let Some(composed_storage_view) = sh.direct_composed_storage_view.as_ref() else {
            return Self::disabled();
        };

        let layout = DirectComposeLayout {
            grid_dimensions: section.grid_dimensions,
            atlas_dimensions: section.atlas_dimensions,
            tile_dimension: section.tile_dimension,
            tile_border: section.tile_border,
            atlas_tiles_per_row: section.atlas_tiles_per_row,
            tiles_per_layer: section.tiles_per_layer,
            atlas_layer_count: section.layer_count,
        };
        let pipeline = build_promotion_pass(
            device,
            sh,
            layout,
            Some(delta),
            weights_buffer,
            composed_storage_view,
        );

        log::info!(
            "[Renderer] Direct SH compose: {} selected-light CSR entr(y/ies), atlas {}×{}",
            delta.affinity_lights.len(),
            section.atlas_dimensions[0],
            section.atlas_dimensions[1],
        );

        Self {
            pipeline: Some(pipeline),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn new_case2(
        device: &wgpu::Device,
        sh: &ShVolumeResources,
        direct_section: Option<&DirectShVolumeSection>,
        delta: Option<&DirectShDeltaVolumesSection>,
        animated_delta: &AnimatedDirectShDeltaVolumesSection,
        weights_buffer: &wgpu::Buffer,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let Some(intermediate_storage_view) = sh.direct_intermediate_storage_view.as_ref() else {
            return Self::disabled();
        };
        let Some(intermediate_sampled_view) = sh.direct_intermediate_sampled_view.as_ref() else {
            return Self::disabled();
        };
        let Some(composed_storage_view) = sh.direct_composed_storage_view.as_ref() else {
            return Self::disabled();
        };
        let Some(layout) = direct_compose_layout(direct_section, sh) else {
            return Self::disabled();
        };

        // Pass A keeps the existing shader unmodified. In Case 2, it accepts
        // absent id35/id41 inputs through the existing dummy base and empty CSR
        // buffers, then writes the intermediate that Pass B always consumes.
        let mut pass_a = build_promotion_pass(
            device,
            sh,
            layout,
            delta,
            weights_buffer,
            intermediate_storage_view,
        );
        pass_a.animated_add = Some(build_animated_direct_pass(
            device,
            sh,
            layout,
            animated_delta,
            intermediate_sampled_view,
            composed_storage_view,
            uniform_bind_group_layout,
        ));

        log::info!(
            "[Renderer] Direct SH compose: Case 2, {} animated-light CSR entr(y/ies), atlas {}×{}",
            animated_delta.affinity_lights.len(),
            layout.atlas_dimensions[0],
            layout.atlas_dimensions[1],
        );

        Self {
            pipeline: Some(pass_a),
        }
    }

    pub fn dispatch_if_needed(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        active: bool,
        debug_overrides: DirectShComposeDebugOverrides,
        timestamp_writes: DirectShComposeTimestampWrites<'_>,
    ) {
        let Some(pipeline) = self.pipeline.as_mut() else {
            return;
        };

        let debug_bytes = debug_override_bytes(debug_overrides.promotion);
        if debug_bytes != pipeline.last_debug_override_bytes {
            queue.write_buffer(&pipeline.debug_override_buffer, 0, &debug_bytes);
            pipeline.last_debug_override_bytes = debug_bytes;
        }
        if let Some(animated_add) = pipeline.animated_add.as_mut() {
            let animated_debug_bytes = debug_overrides.animated.bytes();
            if animated_debug_bytes != animated_add.last_debug_override_bytes {
                queue.write_buffer(
                    &animated_add.debug_override_buffer,
                    0,
                    &animated_debug_bytes,
                );
                animated_add.last_debug_override_bytes = animated_debug_bytes;
            }
        }

        if !direct_compose_should_dispatch(
            active,
            pipeline.pending_copy_through,
            pipeline.was_active,
        ) {
            return;
        }

        let wg_x = pipeline.dispatch_dimensions[0].div_ceil(8).max(1);
        let wg_y = pipeline.dispatch_dimensions[1].div_ceil(8).max(1);
        let wg_z = pipeline.dispatch_dimensions[2].max(1);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Direct SH Compose"),
                timestamp_writes: timestamp_writes.promotion,
            });
            pass.set_pipeline(&pipeline.pipeline);
            pass.set_bind_group(0, &pipeline.bind_group, &[]);
            pass.dispatch_workgroups(wg_x, wg_y, wg_z);
        }
        if let Some(animated_add) = &pipeline.animated_add {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Animated Direct SH Compose"),
                timestamp_writes: timestamp_writes.animated,
            });
            pass.set_pipeline(&animated_add.pipeline);
            pass.set_bind_group(0, uniform_bind_group, &[]);
            pass.set_bind_group(1, &animated_add.bind_group, &[]);
            pass.dispatch_workgroups(wg_x, wg_y, wg_z);
        }
        pipeline.pending_copy_through = false;
        pipeline.was_active = active;
    }
}

fn direct_compose_layout(
    direct_section: Option<&DirectShVolumeSection>,
    sh: &ShVolumeResources,
) -> Option<DirectComposeLayout> {
    if let Some(section) = direct_section {
        return Some(DirectComposeLayout {
            grid_dimensions: section.grid_dimensions,
            atlas_dimensions: section.atlas_dimensions,
            tile_dimension: section.tile_dimension,
            tile_border: section.tile_border,
            atlas_tiles_per_row: section.atlas_tiles_per_row,
            tiles_per_layer: section.tiles_per_layer,
            atlas_layer_count: section.layer_count,
        });
    }
    sh.present.then_some(DirectComposeLayout {
        grid_dimensions: sh.grid_dimensions,
        atlas_dimensions: sh.atlas_dimensions,
        tile_dimension: sh.tile_dimension,
        tile_border: sh.tile_border,
        atlas_tiles_per_row: sh.atlas_tiles_per_row,
        tiles_per_layer: sh.tiles_per_layer,
        atlas_layer_count: sh.atlas_layer_count,
    })
}

fn build_promotion_pass(
    device: &wgpu::Device,
    sh: &ShVolumeResources,
    layout: DirectComposeLayout,
    delta: Option<&DirectShDeltaVolumesSection>,
    weights_buffer: &wgpu::Buffer,
    output_storage_view: &wgpu::TextureView,
) -> DirectShComposePipeline {
    let storage = DirectPromotionStorage::new(delta, layout.grid_dimensions);
    // The instrumentation covers both cases. The promotion pass binds only
    // id-41 storage; runtime weights and Case 2's id-45 pass are excluded.
    #[cfg(feature = "dev-tools")]
    storage.log_footprint();
    let DirectPromotionStorage {
        buffers,
        subblock_bytes,
        offsets_bytes,
        lights_bytes,
    } = storage;

    use wgpu::util::DeviceExt;
    let delta_subblocks_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Direct SH Compose Delta Subblocks (f16)"),
        contents: &subblock_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let affinity_offsets_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Direct SH Compose Affinity Offsets"),
        contents: &offsets_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let affinity_lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Direct SH Compose Affinity Lights"),
        contents: &lights_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });

    let grid_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Direct SH Compose Grid Dims"),
        contents: &build_compose_grid_bytes(ComposeGridParams {
            grid_dimensions: layout.grid_dimensions,
            atlas_dimensions: layout.atlas_dimensions,
            tile_dimension: layout.tile_dimension,
            tile_border: layout.tile_border,
            atlas_tiles_per_row: layout.atlas_tiles_per_row,
            tiles_per_layer: layout.tiles_per_layer,
            atlas_layer_count: layout.atlas_layer_count,
            affinity_dims: buffers.affinity_dims,
            // Direct compose retains the dense base-atlas geometry, so the
            // compact id-34 tail words are intentionally unused here.
            compact_atlas_tiles_per_row: 0,
            compact_atlas_tiles_per_layer: 0,
        }),
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
        entries: &promotion_compose_bgl_entries(),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Direct SH Compose Pipeline Layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Direct SH Compose Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/direct_sh_compose.wgsl").into()),
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
    let base_sampler = nearest_sampler(device, "Direct SH Compose Base Atlas Sampler");
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

    DirectShComposePipeline {
        pipeline,
        bind_group,
        dispatch_dimensions: [
            layout.atlas_dimensions[0],
            layout.atlas_dimensions[1],
            layout.atlas_layer_count,
        ],
        debug_override_buffer,
        pending_copy_through: true,
        was_active: false,
        last_debug_override_bytes: debug_override_bytes,
        animated_add: None,
    }
}

pub(super) fn nearest_sampler(device: &wgpu::Device, label: &str) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some(label),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    })
}

fn promotion_compose_bgl_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        texture_bgl_entry(0),
        sampler_bgl_entry(BIND_BASE_SAMPLER),
        storage_texture_bgl_entry(1),
        uniform_bgl_entry(18),
        storage_bgl_entry(BIND_DELTA_SUBBLOCKS),
        storage_bgl_entry(BIND_AFFINITY_OFFSETS),
        storage_bgl_entry(BIND_AFFINITY_LIGHTS),
        storage_bgl_entry(BIND_SELECTION_WEIGHTS),
        uniform_bgl_entry(BIND_DEBUG_OVERRIDE),
    ]
}

pub(super) fn texture_bgl_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    }
}

pub(super) fn sampler_bgl_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
        count: None,
    }
}

pub(super) fn storage_texture_bgl_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: wgpu::TextureFormat::Rgba16Float,
            view_dimension: wgpu::TextureViewDimension::D2Array,
        },
        count: None,
    }
}

pub(super) fn uniform_bgl_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
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

pub(super) fn storage_bgl_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
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

    #[cfg(feature = "dev-tools")]
    use log::Level;
    #[cfg(feature = "dev-tools")]
    use postretro_level_format::delta_sh_volumes::{
        AFFINITY_FACTOR, DEFAULT_DELTA_PROBE_F16_STRIDE, PROBES_PER_CELL,
    };
    #[cfg(feature = "dev-tools")]
    use postretro_level_format::octahedral::{
        DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
    };
    #[cfg(feature = "dev-tools")]
    use postretro_test_log_capture::LogCapture;

    #[cfg(feature = "dev-tools")]
    fn direct_delta_fixture() -> DirectShDeltaVolumesSection {
        DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: vec![u64::MAX],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: vec![0; PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE],
        }
    }

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
        assert!((f32::from_ne_bytes(bytes[16..20].try_into().unwrap()) - 0.5).abs() < f32::EPSILON);
        assert!(bytes[8..16].iter().all(|&byte| byte == 0));
        assert!(bytes[20..32].iter().all(|&byte| byte == 0));
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn direct_promotion_logs_id_41_bound_storage_once() {
        let section = direct_delta_fixture();
        let storage = DirectPromotionStorage::new(Some(&section), [4, 4, 4]);
        assert_eq!(
            storage.footprint(),
            ComposeStorageFootprint {
                delta_subblocks_bytes: 18_432,
                affinity_offsets_bytes: 8,
                affinity_lights_bytes: 4,
                animation_descriptor_indices_bytes: 0,
            }
        );
        let capture = LogCapture::start();

        storage.log_footprint();

        capture.assert_logged_once(
            Level::Info,
            "[Renderer] DIRECT SH compose id-41 promotion @group(0) storage footprint:",
        );
        capture.assert_not_logged(Level::Info, "SH compose @group(1) storage footprint:");
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn direct_promotion_case_2_stub_logs_actual_bound_sizes_once() {
        // Case 2 may have id 45 without id 41. Its promotion pass still binds
        // two four-byte dummy payloads and the dense zeroed offsets table.
        let storage = DirectPromotionStorage::new(None, [5, 2, 1]);
        assert_eq!(
            storage.footprint(),
            ComposeStorageFootprint {
                delta_subblocks_bytes: 4,
                affinity_offsets_bytes: 12,
                affinity_lights_bytes: 4,
                animation_descriptor_indices_bytes: 0,
            }
        );
        let capture = LogCapture::start();

        storage.log_footprint();

        capture.assert_logged_once(
            Level::Info,
            "DIRECT SH compose id-41 promotion @group(0) storage footprint: delta_subblocks 0.00 MiB (4 B), affinity_offsets 0.00 MiB (12 B), affinity_lights 0.00 MiB (4 B), animation_descriptor_indices 0.00 MiB (0 B) - total 0.00 MiB (20 B)",
        );
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
        assert!(
            module
                .entry_points
                .iter()
                .any(|entry| entry.name == "compose_main"
                    && entry.stage == naga::ShaderStage::Compute)
        );
    }
}
