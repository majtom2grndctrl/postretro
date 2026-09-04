// Direct SH compose passes: promotion subtraction plus optional animated-direct addition.
// See: context/lib/rendering_pipeline.md §7.1

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_render_cpu::frame_uniforms::LightTermMask;
#[cfg(feature = "dev-tools")]
use postretro_render_cpu::sh_compose::ComposeStorageFootprint;
use postretro_render_cpu::sh_compose::{
    ComposeGridParams, DirectDeltaComposeBuffers, build_compose_grid_bytes,
    build_direct_delta_buffers, pad_storage_bytes, u16_slice_to_bytes, u32_slice_to_bytes,
};

use super::animated_direct_sh_compose::{
    AnimatedDirectShComposePipeline, AnimatedDirectShDebugOverride, build_animated_direct_pass,
};
use super::direct_sh_resources::{DirectAtlasLayout, DirectShResources};
use super::sh_indirection::probe_indirection_storage_bytes;
use super::sh_volume::AnimatedLightBuffers;

pub(super) const BIND_BASE_SAMPLER: u32 = 2;
pub(super) const BIND_DELTA_SUBBLOCKS: u32 = 20;
pub(super) const BIND_AFFINITY_OFFSETS: u32 = 21;
pub(super) const BIND_ANIMATION_DESCRIPTORS: u32 = 22;
pub(super) const BIND_ANIMATION_SAMPLES: u32 = 23;
pub(super) const BIND_AFFINITY_LIGHTS: u32 = 24;
pub(super) const BIND_ANIMATION_DESCRIPTOR_INDICES: u32 = 25;
const BIND_SELECTION_WEIGHTS: u32 = 26;
const BIND_DEBUG_OVERRIDE: u32 = 27;
pub(super) const BIND_DELTA_COMPACTION_META: u32 = 28;
/// Bindings 0..=28 are the existing Pass-A contract; append the private
/// per-frame mask instead of changing any established binding.
const BIND_FRAME_LIGHT_TERM_MASK: u32 = 29;
/// Load-derived id-34 indirection words. The shader starts consuming this in
/// Task 4; reserve and populate the carrier now so all compose paths receive
/// byte-identical words from one builder.
pub(super) const BIND_PROBE_INDIRECTION: u32 = 30;
const DEBUG_OVERRIDE_SIZE: usize = 32;
/// WGSL `DirectComposeParams`: mask at byte 0, followed by three u32 pads.
const DIRECT_COMPOSE_PARAMS_SIZE: usize = 16;
const _: () = assert!(DIRECT_COMPOSE_PARAMS_SIZE == 16);
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

/// Inputs captured once for both direct-compose passes during scene recording.
/// Keeping them together prevents Pass A's private mask from drifting from
/// Pass B's shared group-0 snapshot.
pub(super) struct DirectShComposeFrameInputs<'a> {
    pub(super) uniform_bind_group: &'a wgpu::BindGroup,
    pub(super) active: bool,
    pub(super) light_term_mask: LightTermMask,
    pub(super) debug_overrides: DirectShComposeDebugOverrides,
    pub(super) timestamp_writes: DirectShComposeTimestampWrites<'a>,
}

struct DirectShComposePipeline {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    /// Affinity-cell dimensions. One 8×8 workgroup reconstructs and writes the
    /// 4×4×4 probe tiles belonging to one brick in both direct compose passes.
    dispatch_dimensions: [u32; 3],
    debug_override_buffer: wgpu::Buffer,
    /// Pass A has no shared group-0 binding, so this mirrors that group's
    /// frame snapshot in a private uniform.
    light_term_mask_buffer: wgpu::Buffer,
    pending_copy_through: bool,
    was_active: bool,
    /// The mask snapshot that produced the currently composed atlas.
    last_composed_mask: LightTermMask,
    last_debug_override_bytes: [u8; DEBUG_OVERRIDE_SIZE],
    /// Present only for section-45 maps. Case 1 retains exactly one pass and
    /// writes the final sampled atlas directly.
    animated_add: Option<AnimatedDirectShComposePipeline>,
}

struct DirectPromotionStorage {
    buffers: DirectDeltaComposeBuffers,
    subblock_bytes: Vec<u8>,
    compaction_meta_bytes: Vec<u8>,
    offsets_bytes: Vec<u8>,
    lights_bytes: Vec<u8>,
}

impl DirectPromotionStorage {
    fn new(delta: Option<&DirectShDeltaVolumesSection>, grid_dimensions: [u32; 3]) -> Self {
        let buffers = build_direct_delta_buffers(delta, grid_dimensions);
        let subblock_bytes = pad_storage_bytes(u16_slice_to_bytes(&buffers.delta_subblocks), 4);
        let compaction_meta_bytes =
            pad_storage_bytes(u32_slice_to_bytes(&buffers.compaction_meta_words()), 4);
        let offsets_bytes = pad_storage_bytes(u32_slice_to_bytes(&buffers.affinity_offsets), 8);
        let lights_bytes = pad_storage_bytes(u32_slice_to_bytes(&buffers.affinity_lights), 4);
        Self {
            buffers,
            subblock_bytes,
            compaction_meta_bytes,
            offsets_bytes,
            lights_bytes,
        }
    }

    #[cfg(feature = "dev-tools")]
    fn footprint(&self) -> ComposeStorageFootprint {
        ComposeStorageFootprint {
            delta_subblocks_bytes: self.subblock_bytes.len(),
            delta_compaction_meta_bytes: self.compaction_meta_bytes.len(),
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
        direct: &DirectShResources,
        animation: &AnimatedLightBuffers,
        probe_indirection_words: &[u32],
        delta: Option<&DirectShDeltaVolumesSection>,
        animated_delta: Option<&AnimatedDirectShDeltaVolumesSection>,
        weights_buffer: &wgpu::Buffer,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        match animated_delta {
            // Case 2 is selected only at level load from section presence.
            Some(animated_delta) => Self::new_case2(
                device,
                direct,
                animation,
                probe_indirection_words,
                delta,
                animated_delta,
                weights_buffer,
                uniform_bind_group_layout,
            ),
            // Section 45 absent: Pass A still performs base copy-through so
            // the static-direct mask works even without promotion deltas.
            None => Self::new_case1(
                device,
                direct,
                probe_indirection_words,
                delta,
                weights_buffer,
            ),
        }
    }

    fn new_case1(
        device: &wgpu::Device,
        direct: &DirectShResources,
        probe_indirection_words: &[u32],
        delta: Option<&DirectShDeltaVolumesSection>,
        weights_buffer: &wgpu::Buffer,
    ) -> Self {
        if !direct.has_direct_base {
            return Self::disabled();
        }
        let delta = delta.filter(|delta| !delta.affinity_lights.is_empty());
        let Some(composed_storage_view) = direct.composed_storage_view.as_ref() else {
            return Self::disabled();
        };
        let Some(layout) = direct.compose_layout else {
            return Self::disabled();
        };
        let pipeline = build_promotion_pass(
            device,
            direct,
            layout,
            probe_indirection_words,
            delta,
            weights_buffer,
            composed_storage_view,
        );

        log::info!(
            "[Renderer] Direct SH compose: {} selected-light CSR entr(y/ies), atlas {}×{}",
            delta.map_or(0, |delta| delta.affinity_lights.len()),
            layout.atlas_dimensions[0],
            layout.atlas_dimensions[1],
        );

        Self {
            pipeline: Some(pipeline),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn new_case2(
        device: &wgpu::Device,
        direct: &DirectShResources,
        animation: &AnimatedLightBuffers,
        probe_indirection_words: &[u32],
        delta: Option<&DirectShDeltaVolumesSection>,
        animated_delta: &AnimatedDirectShDeltaVolumesSection,
        weights_buffer: &wgpu::Buffer,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let Some(intermediate_storage_view) = direct.intermediate_storage_view.as_ref() else {
            return Self::disabled();
        };
        let Some(intermediate_sampled_view) = direct.intermediate_sampled_view.as_ref() else {
            return Self::disabled();
        };
        let Some(composed_storage_view) = direct.composed_storage_view.as_ref() else {
            return Self::disabled();
        };
        let Some(layout) = direct.compose_layout else {
            return Self::disabled();
        };

        // Pass A keeps the existing shader unmodified. In Case 2, it accepts
        // absent id35/id41 inputs through the existing dummy base and empty CSR
        // buffers, then writes the intermediate that Pass B always consumes.
        let mut pass_a = build_promotion_pass(
            device,
            direct,
            layout,
            probe_indirection_words,
            delta,
            weights_buffer,
            intermediate_storage_view,
        );
        pass_a.animated_add = Some(build_animated_direct_pass(
            device,
            animation,
            layout,
            probe_indirection_words,
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
        frame: DirectShComposeFrameInputs<'_>,
    ) {
        let DirectShComposeFrameInputs {
            uniform_bind_group,
            active,
            light_term_mask: frame_light_term_mask,
            debug_overrides,
            timestamp_writes,
        } = frame;
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
            frame_light_term_mask,
            pipeline.last_composed_mask,
        ) {
            return;
        }

        // Keep Pass A's private input coherent with Pass B's group-0 input.
        // This applies to the initial copy-through too, never relying on the
        // construction-time default mask.
        let light_term_mask_bytes = direct_compose_params_bytes(frame_light_term_mask);
        queue.write_buffer(&pipeline.light_term_mask_buffer, 0, &light_term_mask_bytes);
        let wg_x = pipeline.dispatch_dimensions[0].max(1);
        let wg_y = pipeline.dispatch_dimensions[1].max(1);
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
        pipeline.last_composed_mask = frame_light_term_mask;
    }
}

fn build_promotion_pass(
    device: &wgpu::Device,
    direct: &DirectShResources,
    layout: DirectAtlasLayout,
    probe_indirection_words: &[u32],
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
        compaction_meta_bytes,
        offsets_bytes,
        lights_bytes,
    } = storage;

    use wgpu::util::DeviceExt;
    let delta_subblocks_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Direct SH Compose Delta Subblocks (f16)"),
        contents: &subblock_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let delta_compaction_meta_buffer =
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Direct SH Compose Delta Compaction Meta"),
            contents: &compaction_meta_bytes,
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
    let probe_indirection_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Direct SH Compose Probe Indirection"),
        contents: &probe_indirection_storage_bytes(probe_indirection_words),
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
            // Retain the fixed 64-byte uniform layout: both field pairs now
            // name the same stored-tile atlas geometry.
            compact_atlas_tiles_per_row: layout.atlas_tiles_per_row,
            compact_atlas_tiles_per_layer: layout.tiles_per_layer,
        }),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let debug_override_bytes = debug_override_bytes(DirectShDebugOverride::default());
    let debug_override_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Direct SH Compose Debug Override"),
        contents: &debug_override_bytes,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let initial_light_term_mask_bytes = direct_compose_params_bytes(LightTermMask::ALL);
    let light_term_mask_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Direct SH Compose Frame Light-Term Mask"),
        contents: &initial_light_term_mask_bytes,
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
                resource: wgpu::BindingResource::TextureView(&direct.base_atlas_view),
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
                binding: BIND_DELTA_COMPACTION_META,
                resource: delta_compaction_meta_buffer.as_entire_binding(),
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
            wgpu::BindGroupEntry {
                binding: BIND_FRAME_LIGHT_TERM_MASK,
                resource: light_term_mask_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_PROBE_INDIRECTION,
                resource: probe_indirection_buffer.as_entire_binding(),
            },
        ],
    });

    DirectShComposePipeline {
        pipeline,
        bind_group,
        dispatch_dimensions: buffers.affinity_dims,
        debug_override_buffer,
        light_term_mask_buffer,
        pending_copy_through: true,
        was_active: false,
        last_composed_mask: LightTermMask::ALL,
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
        storage_bgl_entry(BIND_DELTA_COMPACTION_META),
        storage_bgl_entry(BIND_AFFINITY_OFFSETS),
        storage_bgl_entry(BIND_AFFINITY_LIGHTS),
        storage_bgl_entry(BIND_SELECTION_WEIGHTS),
        uniform_bgl_entry(BIND_DEBUG_OVERRIDE),
        direct_compose_params_bgl_entry(),
        storage_bgl_entry(BIND_PROBE_INDIRECTION),
    ]
}

fn direct_compose_params_bgl_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: BIND_FRAME_LIGHT_TERM_MASK,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(DIRECT_COMPOSE_PARAMS_SIZE as u64),
        },
        count: None,
    }
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

fn direct_compose_params_bytes(mask: LightTermMask) -> [u8; DIRECT_COMPOSE_PARAMS_SIZE] {
    let mut bytes = [0u8; DIRECT_COMPOSE_PARAMS_SIZE];
    bytes[0..4].copy_from_slice(&mask.bits().to_ne_bytes());
    bytes
}

fn direct_compose_should_dispatch(
    active: bool,
    pending_copy_through: bool,
    was_active: bool,
    frame_light_term_mask: LightTermMask,
    last_composed_mask: LightTermMask,
) -> bool {
    active || pending_copy_through || was_active || frame_light_term_mask != last_composed_mask
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
            cell_levels: vec![0u8; 1],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: vec![0; PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE],
        }
    }

    #[test]
    fn direct_compose_schedules_load_active_and_zero_transition() {
        assert!(direct_compose_should_dispatch(
            false,
            true,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(direct_compose_should_dispatch(
            true,
            false,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(direct_compose_should_dispatch(
            false,
            false,
            true,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(!direct_compose_should_dispatch(
            false,
            false,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
    }

    #[test]
    fn direct_compose_mask_change_and_return_to_all_re_dirty() {
        // Ordering T1: the predicate compares against the last composed mask,
        // including when the newly selected value is the all-on default.
        let mut static_direct_off = LightTermMask::ALL;
        static_direct_off.set_enabled(LightTermMask::BAKED_DIRECT_STATIC, false);

        assert!(direct_compose_should_dispatch(
            false,
            false,
            false,
            static_direct_off,
            LightTermMask::ALL,
        ));
        assert!(direct_compose_should_dispatch(
            false,
            false,
            false,
            LightTermMask::ALL,
            static_direct_off,
        ));
    }

    #[test]
    fn direct_compose_mask_change_stays_dirty_until_a_world_dispatch_records_it() {
        // Ordering T8: a skipped non-world frame cannot consume this dirty state.
        let mut animated_direct_off = LightTermMask::ALL;
        animated_direct_off.set_enabled(LightTermMask::BAKED_DIRECT_ANIMATED, false);
        let last_composed_mask = LightTermMask::ALL;

        // A non-world frame does not call dispatch_if_needed, so this unchanged
        // stored value must still make the next world frame dispatch.
        assert!(direct_compose_should_dispatch(
            false,
            false,
            false,
            animated_direct_off,
            last_composed_mask,
        ));
        assert!(direct_compose_should_dispatch(
            false,
            false,
            false,
            animated_direct_off,
            last_composed_mask,
        ));
        assert!(!direct_compose_should_dispatch(
            false,
            false,
            false,
            animated_direct_off,
            animated_direct_off,
        ));
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
                delta_compaction_meta_bytes: 16,
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
                delta_compaction_meta_bytes: 24,
                affinity_offsets_bytes: 12,
                affinity_lights_bytes: 4,
                animation_descriptor_indices_bytes: 0,
            }
        );
        let capture = LogCapture::start();

        storage.log_footprint();

        capture.assert_logged_once(
            Level::Info,
            "DIRECT SH compose id-41 promotion @group(0) storage footprint: delta_subblocks 0.00 MiB (4 B), delta_compaction_meta 0.00 MiB (24 B), affinity_offsets 0.00 MiB (12 B), affinity_lights 0.00 MiB (4 B), animation_descriptor_indices 0.00 MiB (0 B) - total 0.00 MiB (44 B)",
        );
    }

    #[test]
    fn base_only_direct_compose_uses_empty_csr_and_static_mask_copy_through() {
        // Regression: without id 41 or id 45, Pass A must remain available so
        // clearing bit 3 writes zero instead of exposing the immutable base.
        let storage = DirectPromotionStorage::new(None, [1, 1, 1]);
        assert!(storage.buffers.delta_subblocks.is_empty());
        assert_eq!(storage.buffers.affinity_offsets, vec![0, 0]);
        assert!(storage.buffers.affinity_lights.is_empty());

        let mut static_direct_off = LightTermMask::ALL;
        static_direct_off.set_enabled(LightTermMask::BAKED_DIRECT_STATIC, false);
        let params = direct_compose_params_bytes(static_direct_off);
        assert_eq!(
            u32::from_ne_bytes(params[0..4].try_into().unwrap())
                & LightTermMask::BAKED_DIRECT_STATIC.bits(),
            0
        );

        let source = include_str!("../shaders/direct_sh_compose.wgsl");
        assert!(source.contains("if (in_grid && use_baked_direct_static)"));
        assert!(source.contains("accum[texel_index] = vec4<f32>(0.0);"));
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

    #[test]
    fn direct_compose_binds_one_combined_delta_compaction_meta_buffer() {
        let entries = promotion_compose_bgl_entries();
        assert!(entries.iter().any(|entry| {
            entry.binding == BIND_DELTA_COMPACTION_META
                && matches!(
                    &entry.ty,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        ..
                    }
                )
        }));
        assert!(entries.iter().any(|entry| {
            entry.binding == BIND_PROBE_INDIRECTION
                && matches!(
                    &entry.ty,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        ..
                    }
                )
        }));
        let storage_count = entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.ty,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { .. },
                        ..
                    }
                )
            })
            .count();
        assert_eq!(storage_count, 6);
    }

    #[test]
    fn direct_compose_appends_a_sixteen_byte_private_frame_mask_uniform() {
        // Ordering T4/T11: Pass A receives the same captured frame mask as the
        // group-0 reader used by Pass B and the world path.
        let entries = promotion_compose_bgl_entries();
        let entry = entries
            .iter()
            .find(|entry| entry.binding == BIND_FRAME_LIGHT_TERM_MASK)
            .expect("Pass A must bind its private per-frame mask input");
        let wgpu::BindingType::Buffer {
            ty,
            min_binding_size,
            ..
        } = &entry.ty
        else {
            panic!("Pass A frame mask must be a uniform buffer");
        };
        assert_eq!(*ty, wgpu::BufferBindingType::Uniform);
        assert_eq!(
            min_binding_size.map(std::num::NonZeroU64::get),
            Some(DIRECT_COMPOSE_PARAMS_SIZE as u64),
        );

        let bytes = direct_compose_params_bytes(LightTermMask::BAKED_DIRECT_STATIC);
        assert_eq!(bytes.len(), DIRECT_COMPOSE_PARAMS_SIZE);
        assert_eq!(u32::from_ne_bytes(bytes[0..4].try_into().unwrap()), 0x08);
        assert!(bytes[4..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn direct_compose_uses_validity_to_gate_delta_reconstruction() {
        let source = include_str!("../shaders/direct_sh_compose.wgsl");
        assert!(
            source.contains(
                "let output_is_valid = in_grid && local_probe_is_valid(cell_index, local_probe);"
            ),
            "id-41 has no base probe-indirection, so its validity mask must gate delta reads"
        );
        assert!(
            !source.contains("enable f16"),
            "direct delta payloads remain Rgba16Float read through f32 unpacking"
        );
    }

    #[test]
    fn direct_compose_uses_private_mask_for_static_base_and_dynamic_subtraction() {
        let source = include_str!("../shaders/direct_sh_compose.wgsl");
        assert!(source.contains("@group(0) @binding(29) var<uniform> direct_compose_params"));
        assert!(source.contains(
            "let use_baked_direct_static = (direct_compose_params.light_term_mask & LIGHT_TERM_BAKED_DIRECT_STATIC) != 0u;"
        ));
        assert!(source.contains("if (in_grid && use_baked_direct_static)"));
        assert!(source.contains(
            "let required_terms = LIGHT_TERM_BAKED_DIRECT_STATIC | LIGHT_TERM_DYNAMIC_DIRECT;"
        ));
        assert!(source.contains(
            "if ((direct_compose_params.light_term_mask & required_terms) != required_terms)"
        ));
        assert!(source.contains(
            "let use_promotion_subtraction = use_baked_direct_static && use_dynamic_direct;"
        ));
    }

    #[test]
    fn static_direct_off_with_dynamic_on_skips_dense_and_coarsened_promotion() {
        // A negative SH delta exposes the original failure: subtracting it
        // from an already-zero accumulator produces a positive coefficient
        // that survives the final clamp. Keep a nonzero promotion weight in
        // this reference case so both compose paths must be blocked by bit 3.
        let mut static_off_dynamic_on = LightTermMask::ALL;
        static_off_dynamic_on.set_enabled(LightTermMask::BAKED_DIRECT_STATIC, false);
        assert!(static_off_dynamic_on.contains(LightTermMask::DYNAMIC_DIRECT));
        let use_baked_direct_static =
            static_off_dynamic_on.contains(LightTermMask::BAKED_DIRECT_STATIC);
        let use_dynamic_direct = static_off_dynamic_on.contains(LightTermMask::DYNAMIC_DIRECT);
        let use_promotion_subtraction = use_baked_direct_static && use_dynamic_direct;
        let promotion_weight = 0.75_f32;

        for coarsened in [false, true] {
            let mut accum = if use_baked_direct_static { 1.0 } else { 0.0 };
            if use_promotion_subtraction && promotion_weight > 0.0 {
                let delta = if coarsened { -0.5 } else { -0.25 };
                accum -= delta * promotion_weight;
            }
            assert_eq!(accum.max(0.0), 0.0);
        }

        let source = include_str!("../shaders/direct_sh_compose.wgsl");
        let dense_start = source
            .find("    if (level == 0u) {")
            .expect("shader must retain the dense L0 path");
        let coarsened_start = source[dense_start..]
            .find("    } else if (use_promotion_subtraction) {")
            .map(|offset| dense_start + offset)
            .expect("bit 3 and bit 5 must uniformly guard the coarsened path");
        let output_start = source[coarsened_start..]
            .find("\n    if (in_grid) {")
            .map(|offset| coarsened_start + offset)
            .expect("shader must retain its output-store path");
        let dense_path = &source[dense_start..coarsened_start];
        let coarsened_path = &source[coarsened_start..output_start];

        assert!(
            dense_path.contains("if (output_is_valid && use_promotion_subtraction)")
                && dense_path.contains("read_delta_texel("),
            "the combined bit-3/bit-5 guard must wrap dense L0 delta reads",
        );
        assert!(
            coarsened_path.starts_with("    } else if (use_promotion_subtraction) {")
                && coarsened_path.contains("read_delta_texel(")
                && coarsened_path.contains("reconstruct_l1_shared_texel("),
            "the uniform combined guard must wrap coarsened reads and reconstruction",
        );
    }

    #[test]
    fn direct_coarsened_compose_uses_one_brick_workgroup_and_kept_shared_tiles() {
        let source = include_str!("../shaders/direct_sh_compose.wgsl");

        assert!(source.contains("@builtin(workgroup_id) brick"));
        assert!(source.contains("var<workgroup> shared_kept_tiles"));
        assert!(source.contains(
            "return grid.affinity_dims.x * grid.affinity_dims.y * grid.affinity_dims.z * 3u"
        ));
        assert!(source.contains("countOneBits(kept_probe_mask_word"));
        assert!(source.contains("if (level == 0u)"));
        assert!(source.contains("if (level == 1u && local_probe_is_kept"));
        assert!(source.contains("if (level == 2u)"));
    }
}
