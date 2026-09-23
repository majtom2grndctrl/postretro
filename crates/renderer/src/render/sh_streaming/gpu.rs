//! GPU-owned backing for streamed SH residency.
//!
//! This module deliberately owns every `wgpu` handle used by the streamed
//! path. The loader passes decoded bytes only; it never observes a slot,
//! texture, buffer, or bind group.

use postretro_level_format::delta_sh_volumes::delta_probe_f16_stride;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::sh_reconstruct::{Level, stored_delta_tiles};
use postretro_level_loader::{ShStreamBaseMetadata, ShStreamSourceMetadata};
use postretro_render_cpu::sh_compose::{ComposeGridParams, DYNAMIC_COMPOSE_GRID_DIMS_SIZE};
use postretro_render_cpu::sh_volume::{
    BIND_ANIM_DESCRIPTORS, BIND_ANIM_SAMPLES, BIND_BILLBOARD_DIRECT_SCATTER,
    BIND_DYNAMIC_DIRECT_PARAMS, BIND_SCRIPTED_LIGHT_DESCRIPTORS, BIND_SH_ATLAS_SAMPLER,
    BIND_SH_DEPTH_MOMENTS, BIND_SH_DIRECT_ATLAS, BIND_SH_GRID_INFO, BIND_SH_TOTAL_ATLAS,
    STREAMED_SH_PHYSICAL_TILE_STRIDE, ShGridInfoParams, build_grid_info_bytes,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use wgpu::util::DeviceExt;

use super::dense::DenseTextures;
use super::direct_compose::{
    DirectSparseReplacement, RetiredDirectSparsePass, StreamingDirectCompose,
    StreamingDirectComposeFrameInputs, StreamingDirectDirtyRanges, StreamingDirectViews,
};
use super::{ShResidencyDrainError, SparseCapacityFloors};
use crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
use crate::render::direct_sh_compose::DirectShDebugOverride;
use crate::render::renderer_types::PromotedBakedLightState;
use crate::render::sh_compose_dispatch::build_dynamic_compose_grid_upload_for_ranges;
use crate::render::sh_indirection::WGSL_DECODE_HELPER;
use crate::render::sh_volume::ShVolumeResources;

mod indirect;

use indirect::{RetiredIndirectSparseResources, StreamingIndirectCompose};

const PHYSICAL_TILE_DIMENSION: u32 = 8;
const BIND_DELTA_SUBBLOCKS: u32 = 20;
const BIND_AFFINITY_OFFSETS: u32 = 21;
const BIND_AFFINITY_LIGHTS: u32 = 24;
const BIND_ANIMATION_DESCRIPTOR_INDICES: u32 = 25;
const BIND_PROBE_INDIRECTION: u32 = 26;
const BIND_DELTA_COMPACTION_META: u32 = 27;
const DIRECT_DELTA_SECTION: u32 = 41;
const ANIMATED_DIRECT_DELTA_SECTION: u32 = 45;

#[derive(Debug, Clone, Copy)]
pub(super) struct AtlasShape {
    pub(super) tiles_per_row: u32,
    pub(super) tiles_per_layer: u32,
    pub(super) layers: u32,
    pub(super) slots: u32,
}

impl AtlasShape {
    fn for_slots(slots: u32, limits: &wgpu::Limits) -> Result<Self, ShResidencyDrainError> {
        let maximum = limits.max_texture_dimension_2d / PHYSICAL_TILE_DIMENSION;
        if maximum == 0 || slots == 0 {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "adapter cannot allocate an 8x8 streamed SH cell",
            });
        }
        let tiles_per_row = integer_sqrt_ceil(slots).min(maximum).max(1);
        // Width/height are fixed for a session. Once a layer is full, capacity
        // grows only by appending array layers, never by changing slot geometry.
        let rows = slots.div_ceil(tiles_per_row).min(maximum).max(1);
        let tiles_per_layer = tiles_per_row
            .checked_mul(rows)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let layers = slots.div_ceil(tiles_per_layer);
        if layers == 0 || layers > limits.max_texture_array_layers {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed SH pool needs unsupported array layers",
            });
        }
        Ok(Self {
            tiles_per_row,
            tiles_per_layer,
            layers,
            // The texture has a complete final layer. Treat its padding as
            // allocatable capacity rather than stranding it behind the
            // requested minimum, while preserving width/height geometry.
            slots: tiles_per_layer
                .checked_mul(layers)
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        })
    }

    pub(super) fn extent(self) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width: self.tiles_per_row * PHYSICAL_TILE_DIMENSION,
            height: (self.tiles_per_layer / self.tiles_per_row) * PHYSICAL_TILE_DIMENSION,
            depth_or_array_layers: self.layers,
        }
    }

    fn grown_for_slots(
        self,
        required_slots: u32,
        limits: &wgpu::Limits,
    ) -> Result<Self, ShResidencyDrainError> {
        if required_slots <= self.slots {
            return Ok(self);
        }
        let minimum_layers = required_slots.div_ceil(self.tiles_per_layer);
        let layers = self
            .layers
            .checked_mul(2)
            .unwrap_or(u32::MAX)
            .max(minimum_layers);
        if layers > limits.max_texture_array_layers {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed dense pool cannot append enough texture-array layers",
            });
        }
        let slots = self
            .tiles_per_layer
            .checked_mul(layers)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        Ok(Self {
            tiles_per_row: self.tiles_per_row,
            tiles_per_layer: self.tiles_per_layer,
            layers,
            slots,
        })
    }
}

/// A sparse family has an independent copy/fence lifetime. Keeping this
/// distinct from the dense atlas retirement prevents a one-row growth from
/// transiently doubling every other streamed SH family.
struct RetiringIndirectGeneration {
    #[allow(dead_code)]
    resources: RetiredIndirectSparseResources,
    capacity_bytes: u64,
    complete: Arc<AtomicBool>,
}

struct RetiringDirectSparseGeneration {
    #[allow(dead_code)]
    pass: RetiredDirectSparsePass,
    capacity_bytes: u64,
    complete: Arc<AtomicBool>,
}

/// The coupled id-34/id-35 atlas generation. Only displaced dense textures
/// and its grid record live through the fence. Submitted command buffers hold
/// any old bind groups themselves; retaining those here would cross-pin the
/// independently grown CSR families.
struct RetiringDenseGeneration {
    #[allow(dead_code)]
    dense: DenseTextures,
    #[allow(dead_code)]
    grid_info: wgpu::Buffer,
    capacity_bytes: u64,
    complete: Arc<AtomicBool>,
}

pub(super) struct StreamingGpuPools {
    pub(super) shape: AtlasShape,
    pub(super) base_format: wgpu::TextureFormat,
    pub(super) direct_format: Option<wgpu::TextureFormat>,
    pub(super) base: wgpu::Texture,
    pub(super) total: wgpu::Texture,
    pub(super) base_view: wgpu::TextureView,
    pub(super) total_storage_view: wgpu::TextureView,
    pub(super) total_sampled_view: wgpu::TextureView,
    pub(super) direct_base: Option<wgpu::Texture>,
    pub(super) direct_base_view: Option<wgpu::TextureView>,
    pub(super) direct_intermediate: Option<wgpu::Texture>,
    pub(super) direct_intermediate_storage_view: Option<wgpu::TextureView>,
    pub(super) direct_intermediate_sampled_view: Option<wgpu::TextureView>,
    pub(super) direct_total: Option<wgpu::Texture>,
    pub(super) direct_total_storage_view: Option<wgpu::TextureView>,
    pub(super) direct_total_sampled_view: Option<wgpu::TextureView>,
    pub(super) depth_moments: wgpu::Texture,
    pub(super) compose_indirection: wgpu::Buffer,
    pub(super) sampled_indirection: wgpu::Buffer,
    pub(super) grid_info: wgpu::Buffer,
    indirect_compose: StreamingIndirectCompose,
    direct_compose: Option<StreamingDirectCompose>,
    has_animated_direct_pass: bool,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) mesh_bind_group: wgpu::BindGroup,
    retiring_dense: Option<RetiringDenseGeneration>,
    retiring_indirect: Option<RetiringIndirectGeneration>,
    retiring_direct_promotion: Option<RetiringDirectSparseGeneration>,
    retiring_direct_animated: Option<RetiringDirectSparseGeneration>,
    pub(super) active_capacity_bytes: u64,
    pub(super) fixed_metadata_bytes: u64,
    pub(super) whole_resident_scatter_bytes: u64,
    effective_floor_bytes: u64,
    probe_occlusion_enabled: bool,
    sparse_capacity_floors: SparseCapacityFloors,
}

impl StreamingGpuPools {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        initial_floor: super::floor::InitialPoolFloor,
        probe_occlusion_enabled: bool,
        sh: &mut ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        selection_weights: &wgpu::Buffer,
    ) -> Result<Self, ShResidencyDrainError> {
        let shape = AtlasShape::for_slots(initial_floor.dense_slots.max(1), &device.limits())?;
        preflight_initial_resource_limits(
            base,
            sources,
            shape,
            &initial_floor.sparse_capacities,
            &device.limits(),
        )?;
        Self::new_with_shape(
            device,
            queue,
            base,
            sources,
            shape,
            &initial_floor.sparse_capacities,
            probe_occlusion_enabled,
            sh,
            uniform_bind_group_layout,
            selection_weights,
            initial_floor.effective_floor_bytes,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_shape(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        shape: AtlasShape,
        sparse_capacity_floors: &SparseCapacityFloors,
        probe_occlusion_enabled: bool,
        sh: &mut ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        selection_weights: &wgpu::Buffer,
        effective_floor_bytes: u64,
    ) -> Result<Self, ShResidencyDrainError> {
        let base_format = texture_format(base.irradiance_format)?;
        let extent = shape.extent();
        let base_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Streamed SH Base Atlas Pool"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: base_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let total_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Streamed SH Total Atlas Pool"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let base_view = array_view(&base_texture, "Streamed SH Base Atlas View");
        let total_storage_view = array_view(&total_texture, "Streamed SH Total Storage View");
        let total_sampled_view = array_view(&total_texture, "Streamed SH Total Sampled View");

        let (direct_base, direct_base_view, direct_format) = if let Some(direct) = &sources.direct {
            let format = texture_format(direct.irradiance_format)?;
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Streamed Direct SH Base Atlas Pool"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = array_view(&texture, "Streamed Direct SH Base Atlas View");
            sh.direct.enable_streamed_atlas(queue);
            (Some(texture), Some(view), Some(format))
        } else {
            (None, None, None)
        };
        let direct_compose_required =
            sources.direct_delta.is_some() || sources.animated_direct_delta.is_some();
        let (
            direct_intermediate,
            direct_intermediate_storage_view,
            direct_intermediate_sampled_view,
            direct_total,
            direct_total_storage_view,
            direct_total_sampled_view,
        ) = if direct_compose_required {
            if direct_base.is_none() {
                return Err(ShResidencyDrainError::GpuCapacity {
                    reason: "streamed direct compose has no id-35 base atlas",
                });
            }
            let total = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Streamed Direct SH Total Atlas Pool"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let total_storage = array_view(&total, "Streamed Direct SH Total Storage View");
            let total_sampled = array_view(&total, "Streamed Direct SH Total Sampled View");
            if sources.animated_direct_delta.is_some() {
                let intermediate = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Streamed Direct SH Intermediate Atlas Pool"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let storage = array_view(
                    &intermediate,
                    "Streamed Direct SH Intermediate Storage View",
                );
                let sampled = array_view(
                    &intermediate,
                    "Streamed Direct SH Intermediate Sampled View",
                );
                (
                    Some(intermediate),
                    Some(storage),
                    Some(sampled),
                    Some(total),
                    Some(total_storage),
                    Some(total_sampled),
                )
            } else {
                (
                    None,
                    None,
                    None,
                    Some(total),
                    Some(total_storage),
                    Some(total_sampled),
                )
            }
        } else {
            (None, None, None, None, None, None)
        };

        let depth_extent = wgpu::Extent3d {
            width: base.grid_dimensions[0].max(1),
            height: base.grid_dimensions[1].max(1),
            depth_or_array_layers: base.grid_dimensions[2].max(1),
        };
        if depth_extent.width > device.limits().max_texture_dimension_3d
            || depth_extent.height > device.limits().max_texture_dimension_3d
            || depth_extent.depth_or_array_layers > device.limits().max_texture_dimension_3d
        {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed SH depth-moment grid exceeds adapter 3D texture limits",
            });
        }
        let depth_moments = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Streamed SH Depth Moments"),
            size: depth_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba16Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth_view = depth_moments.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Streamed SH Depth Moment View"),
            dimension: Some(wgpu::TextureViewDimension::D3),
            ..Default::default()
        });

        let indirection_len = u64::try_from(base.probes.len())
            .ok()
            .and_then(|count| count.checked_mul(u64::from(std::mem::size_of::<u32>() as u32)))
            .ok_or(ShResidencyDrainError::SlotOverflow)?
            .max(4);
        let compose_indirection = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Streamed SH Compose Indirection"),
            size: indirection_len,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let sampled_indirection = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Streamed SH Sampled Indirection Mirror"),
            size: indirection_len,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let grid_bytes = build_grid_info_bytes(ShGridInfoParams {
            grid_origin: base.grid_origin,
            cell_size: base.cell_size,
            grid_dimensions: base.grid_dimensions,
            atlas_dimensions: [extent.width, extent.height],
            tile_dimension: base.tile_dimension,
            tile_border: base.tile_border,
            atlas_tiles_per_row: shape.tiles_per_row,
            physical_tile_stride: STREAMED_SH_PHYSICAL_TILE_STRIDE,
            tiles_per_layer: shape.tiles_per_layer,
            atlas_layer_count: shape.layers,
            present: true,
            probe_occlusion_enabled,
        });
        let grid_info = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Streamed SH Grid Info"),
            contents: &grid_bytes,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let indirect_compose = StreamingIndirectCompose::new(
            device,
            base,
            sources,
            shape,
            &base_view,
            &total_storage_view,
            &compose_indirection,
            sh,
            uniform_bind_group_layout,
            sparse_capacity_floors.get(&27).copied(),
        )?;
        let direct_compose = if direct_compose_required {
            Some(StreamingDirectCompose::new(
                device,
                base,
                sources,
                shape,
                StreamingDirectViews {
                    base: direct_base_view
                        .as_ref()
                        .ok_or(ShResidencyDrainError::GpuCapacity {
                            reason: "streamed direct compose lost its id-35 base view",
                        })?,
                    intermediate_storage: direct_intermediate_storage_view.as_ref(),
                    intermediate_sampled: direct_intermediate_sampled_view.as_ref(),
                    total_storage: direct_total_storage_view.as_ref().ok_or(
                        ShResidencyDrainError::GpuCapacity {
                            reason: "streamed direct compose lost its total storage view",
                        },
                    )?,
                    selection_weights,
                },
                &compose_indirection,
                sh,
                uniform_bind_group_layout,
                sparse_capacity_floors,
            )?)
        } else {
            None
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Streamed SH Atlas Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let direct_view = direct_total_sampled_view
            .as_ref()
            .or(direct_base_view.as_ref())
            .unwrap_or(&sh.direct.atlas_view);
        let entries = vec![
            wgpu::BindGroupEntry {
                binding: BIND_SH_TOTAL_ATLAS,
                resource: wgpu::BindingResource::TextureView(&total_sampled_view),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SH_ATLAS_SAMPLER,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SH_GRID_INFO,
                resource: grid_info.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_ANIM_DESCRIPTORS,
                resource: sh.animation.descriptors.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_ANIM_SAMPLES,
                resource: sh.animation.anim_samples.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SCRIPTED_LIGHT_DESCRIPTORS,
                resource: sh.scripted_light_descriptors.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SH_DEPTH_MOMENTS,
                resource: wgpu::BindingResource::TextureView(&depth_view),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SH_DIRECT_ATLAS,
                resource: wgpu::BindingResource::TextureView(direct_view),
            },
            wgpu::BindGroupEntry {
                binding: BIND_BILLBOARD_DIRECT_SCATTER,
                resource: wgpu::BindingResource::TextureView(
                    &sh.billboard_direct_scatter.sampled_view,
                ),
            },
        ];
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Streamed SH Volume Bind Group"),
            layout: &sh.bind_group_layout,
            entries: &entries,
        });
        let mut mesh_entries = entries;
        mesh_entries.push(wgpu::BindGroupEntry {
            binding: BIND_DYNAMIC_DIRECT_PARAMS,
            resource: sh.direct.dynamic_direct_params_binding(),
        });
        let mesh_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Streamed SH Mesh Bind Group"),
            layout: &sh.mesh_bind_group_layout,
            entries: &mesh_entries,
        });

        let active_capacity_bytes = texture_bytes(base_format, extent)
            .checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Float, extent))
            .and_then(|bytes| {
                direct_format.map_or(Some(bytes), |format| {
                    bytes.checked_add(texture_bytes(format, extent))
                })
            })
            .and_then(|bytes| {
                direct_total.as_ref().map_or(Some(bytes), |_| {
                    bytes.checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Float, extent))
                })
            })
            .and_then(|bytes| {
                direct_intermediate.as_ref().map_or(Some(bytes), |_| {
                    bytes.checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Float, extent))
                })
            })
            .and_then(|bytes| bytes.checked_add(indirect_compose.active_pool_bytes()))
            .and_then(|bytes| {
                bytes.checked_add(
                    direct_compose
                        .as_ref()
                        .map_or(0, StreamingDirectCompose::active_capacity_bytes),
                )
            })
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let fixed_metadata_bytes = indirection_len
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(96))
            .and_then(|bytes| {
                bytes.checked_add(texture_bytes(
                    wgpu::TextureFormat::Rgba16Uint,
                    wgpu::Extent3d {
                        width: base.grid_dimensions[0].max(1),
                        height: base.grid_dimensions[1].max(1),
                        depth_or_array_layers: base.grid_dimensions[2].max(1),
                    },
                ))
            })
            .and_then(|bytes| bytes.checked_add(indirect_compose.fixed_metadata_bytes()))
            .and_then(|bytes| {
                bytes.checked_add(
                    direct_compose
                        .as_ref()
                        .map_or(0, StreamingDirectCompose::fixed_metadata_bytes),
                )
            })
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        // Animation/sample/scripted descriptor buffers are shared immutable
        // ShVolumeResources allocations. They participate in the initial
        // 256 MiB floor calculation, but are already represented by static
        // ledger rows and must not be charged again in the live streaming
        // summary.

        Ok(Self {
            shape,
            base_format,
            direct_format,
            base: base_texture,
            total: total_texture,
            base_view,
            total_storage_view,
            total_sampled_view,
            direct_base,
            direct_base_view,
            direct_intermediate,
            direct_intermediate_storage_view,
            direct_intermediate_sampled_view,
            direct_total,
            direct_total_storage_view,
            direct_total_sampled_view,
            depth_moments,
            compose_indirection,
            sampled_indirection,
            grid_info,
            indirect_compose,
            direct_compose,
            has_animated_direct_pass: sources.animated_direct_delta.is_some(),
            bind_group,
            mesh_bind_group,
            retiring_dense: None,
            retiring_indirect: None,
            retiring_direct_promotion: None,
            retiring_direct_animated: None,
            active_capacity_bytes,
            fixed_metadata_bytes,
            whole_resident_scatter_bytes: sh.billboard_direct_scatter.capacity_bytes(),
            effective_floor_bytes,
            probe_occlusion_enabled,
            sparse_capacity_floors: sparse_capacity_floors.clone(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn grow_dense(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        required_slots: u32,
        probe_occlusion_enabled: bool,
        sh: &mut ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        selection_weights: &wgpu::Buffer,
    ) -> Result<(), ShResidencyDrainError> {
        if required_slots <= self.shape.slots {
            return Ok(());
        }
        let shape = self
            .shape
            .grown_for_slots(required_slots, &device.limits())?;
        if self.retiring_dense.is_some() {
            return Err(ShResidencyDrainError::GpuRetirementPressure {
                family: "dense atlas",
            });
        }
        let current_indirect_active = self.indirect_compose.active_pool_bytes();
        let current_direct_active = self
            .direct_compose
            .as_ref()
            .map_or(0, StreamingDirectCompose::active_capacity_bytes);
        let replacement_active_capacity =
            self.active_capacity_for(shape, current_indirect_active, current_direct_active)?;
        let retiring_dense_capacity = self
            .active_capacity_bytes
            .checked_sub(current_indirect_active)
            .and_then(|bytes| bytes.checked_sub(current_direct_active))
            .and_then(|bytes| bytes.checked_add(self.grid_info.size()))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let replacement = DenseTextures::new(device, queue, base, sources, shape, sh)?;
        let replacement_grid_info = create_grid_info(device, base, shape, probe_occlusion_enabled);
        let (replacement_bind_group, replacement_mesh_bind_group) = create_sample_bind_groups(
            device,
            sh,
            &replacement,
            &replacement_grid_info,
            &self.depth_moments,
        );
        // Validate every fallible direct view requirement before either
        // compose carrier is rebound to the replacement atlas.
        let direct_views = self
            .direct_compose
            .as_ref()
            .map(|_| {
                if self.has_animated_direct_pass
                    && (replacement.direct_intermediate_storage_view.is_none()
                        || replacement.direct_intermediate_sampled_view.is_none())
                {
                    return Err(ShResidencyDrainError::GpuCapacity {
                        reason: "streamed dense growth lost its animated direct intermediate atlas",
                    });
                }
                Ok(StreamingDirectViews {
                    base: replacement.direct_base_view.as_ref().ok_or(
                        ShResidencyDrainError::GpuCapacity {
                            reason: "streamed dense growth lost its id-35 base atlas",
                        },
                    )?,
                    intermediate_storage: replacement.direct_intermediate_storage_view.as_ref(),
                    intermediate_sampled: replacement.direct_intermediate_sampled_view.as_ref(),
                    total_storage: replacement.direct_total_storage_view.as_ref().ok_or(
                        ShResidencyDrainError::GpuCapacity {
                            reason: "streamed dense growth lost its direct compose output atlas",
                        },
                    )?,
                    selection_weights,
                })
            })
            .transpose()?;

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Streamed SH Dense Atlas Growth Copy"),
        });
        let old_extent = self.shape.extent();
        copy_texture(&mut encoder, &self.base, &replacement.base, old_extent);
        copy_texture(&mut encoder, &self.total, &replacement.total, old_extent);
        if let (Some(source), Some(destination)) = (&self.direct_base, &replacement.direct_base) {
            copy_texture(&mut encoder, source, destination, old_extent);
        }
        if let (Some(source), Some(destination)) =
            (&self.direct_intermediate, &replacement.direct_intermediate)
        {
            copy_texture(&mut encoder, source, destination, old_extent);
        }
        if let (Some(source), Some(destination)) = (&self.direct_total, &replacement.direct_total) {
            copy_texture(&mut encoder, source, destination, old_extent);
        }

        // Submitted command buffers retain their old bindings.  Do not put
        // those bindings in the dense retirement ticket: an indirect/direct
        // bind group would cross-pin independently grown sparse families.
        let _old_indirect_bind_group = self.indirect_compose.rebind_dense(
            device,
            shape,
            &replacement.base_view,
            &replacement.total_storage_view,
            &self.compose_indirection,
            sh,
        );
        let _old_direct_bind_groups = if let (Some(direct), Some(views)) =
            (self.direct_compose.as_mut(), direct_views)
        {
            Some(
                direct
                    .rebind_dense(device, shape, views, &self.compose_indirection, sh)
                    .expect("prevalidated dense direct views make bind-group rewiring infallible"),
            )
        } else {
            None
        };

        let old = DenseTextures {
            base_format: std::mem::replace(&mut self.base_format, replacement.base_format),
            direct_format: std::mem::replace(&mut self.direct_format, replacement.direct_format),
            base: std::mem::replace(&mut self.base, replacement.base),
            total: std::mem::replace(&mut self.total, replacement.total),
            base_view: std::mem::replace(&mut self.base_view, replacement.base_view),
            total_storage_view: std::mem::replace(
                &mut self.total_storage_view,
                replacement.total_storage_view,
            ),
            total_sampled_view: std::mem::replace(
                &mut self.total_sampled_view,
                replacement.total_sampled_view,
            ),
            direct_base: std::mem::replace(&mut self.direct_base, replacement.direct_base),
            direct_base_view: std::mem::replace(
                &mut self.direct_base_view,
                replacement.direct_base_view,
            ),
            direct_intermediate: std::mem::replace(
                &mut self.direct_intermediate,
                replacement.direct_intermediate,
            ),
            direct_intermediate_storage_view: std::mem::replace(
                &mut self.direct_intermediate_storage_view,
                replacement.direct_intermediate_storage_view,
            ),
            direct_intermediate_sampled_view: std::mem::replace(
                &mut self.direct_intermediate_sampled_view,
                replacement.direct_intermediate_sampled_view,
            ),
            direct_total: std::mem::replace(&mut self.direct_total, replacement.direct_total),
            direct_total_storage_view: std::mem::replace(
                &mut self.direct_total_storage_view,
                replacement.direct_total_storage_view,
            ),
            direct_total_sampled_view: std::mem::replace(
                &mut self.direct_total_sampled_view,
                replacement.direct_total_sampled_view,
            ),
        };
        let old_grid_info = std::mem::replace(&mut self.grid_info, replacement_grid_info);
        let _old_bind_group = std::mem::replace(&mut self.bind_group, replacement_bind_group);
        let _old_mesh_bind_group =
            std::mem::replace(&mut self.mesh_bind_group, replacement_mesh_bind_group);
        self.shape = shape;
        self.probe_occlusion_enabled = probe_occlusion_enabled;
        self.active_capacity_bytes = replacement_active_capacity;

        queue.submit(std::iter::once(encoder.finish()));
        let complete = Arc::new(AtomicBool::new(false));
        let callback_complete = Arc::clone(&complete);
        queue.on_submitted_work_done(move || {
            callback_complete.store(true, Ordering::Release);
        });
        self.retiring_dense = Some(RetiringDenseGeneration {
            capacity_bytes: retiring_dense_capacity,
            dense: old,
            grid_info: old_grid_info,
            complete,
        });
        let _ = uniform_bind_group_layout;
        Ok(())
    }

    /// Append-preservingly grow one or more sparse families.  A replacement
    /// keeps the dense slot geometry stable, copies every live GPU-visible
    /// row and indirection word, and retains the old generation through the
    /// submission fence just like dense layer growth.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn grow_sparse(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        required: &SparseCapacityFloors,
        sh: &mut ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        selection_weights: &wgpu::Buffer,
    ) -> Result<(), ShResidencyDrainError> {
        let mut floors = self.sparse_capacity_floors.clone();
        let mut sections_to_grow = Vec::new();
        for (&section_id, &(required_entries, required_tiles)) in required {
            let current_entries = self.sparse_entry_capacity(section_id);
            let current_tiles = self.sparse_tile_f16_capacity(section_id);
            if required_entries <= current_entries && required_tiles <= current_tiles {
                continue;
            }
            let waiting_for_retirement = sparse_family_has_retiring_generation(
                section_id,
                self.retiring_indirect.is_some(),
                self.retiring_direct_promotion.is_some(),
                self.retiring_direct_animated.is_some(),
            );
            if waiting_for_retirement {
                return Err(ShResidencyDrainError::GpuRetirementPressure {
                    family: "sparse compose",
                });
            }
            let grown_entries = current_entries
                .checked_mul(2)
                .ok_or(ShResidencyDrainError::SlotOverflow)?
                .max(required_entries);
            let grown_tiles = current_tiles
                .checked_mul(2)
                .ok_or(ShResidencyDrainError::SlotOverflow)?
                .max(required_tiles);
            floors.insert(section_id, (grown_entries, grown_tiles));
            sections_to_grow.push(section_id);
        }
        if sections_to_grow.is_empty() {
            return Ok(());
        }
        // Construct every replacement before encoding a copy or swapping an
        // active family. A later constructor failure must leave all active
        // buffers and their CPU allocation mirrors untouched.
        let indirect_replacement = if sections_to_grow.contains(&27) {
            let floor = *floors.get(&27).ok_or(ShResidencyDrainError::SlotOverflow)?;
            Some(StreamingIndirectCompose::new(
                device,
                base,
                sources,
                self.shape,
                &self.base_view,
                &self.total_storage_view,
                &self.compose_indirection,
                sh,
                uniform_bind_group_layout,
                Some(floor),
            )?)
        } else {
            None
        };
        let mut promotion_replacement = None;
        let mut animated_replacement = None;
        if sections_to_grow.iter().any(|&section| {
            matches!(
                section,
                DIRECT_DELTA_SECTION | ANIMATED_DIRECT_DELTA_SECTION
            )
        }) {
            let views = StreamingDirectViews {
                base: self
                    .direct_base_view
                    .as_ref()
                    .ok_or(ShResidencyDrainError::GpuCapacity {
                        reason: "streamed direct sparse growth lost its id-35 base view",
                    })?,
                intermediate_storage: self.direct_intermediate_storage_view.as_ref(),
                intermediate_sampled: self.direct_intermediate_sampled_view.as_ref(),
                total_storage: self.direct_total_storage_view.as_ref().ok_or(
                    ShResidencyDrainError::GpuCapacity {
                        reason: "streamed direct sparse growth lost its total view",
                    },
                )?,
                selection_weights,
            };
            let direct =
                self.direct_compose
                    .as_ref()
                    .ok_or(ShResidencyDrainError::MalformedChunk {
                        cluster_id: 0,
                        reason: "sparse direct growth requested without a direct compose pass",
                    })?;
            for &section_id in &sections_to_grow {
                if !matches!(
                    section_id,
                    DIRECT_DELTA_SECTION | ANIMATED_DIRECT_DELTA_SECTION
                ) {
                    continue;
                }
                let floor = *floors
                    .get(&section_id)
                    .ok_or(ShResidencyDrainError::SlotOverflow)?;
                let replacement = direct.build_sparse_replacement(
                    device,
                    base,
                    sources,
                    self.shape,
                    StreamingDirectViews {
                        base: views.base,
                        intermediate_storage: views.intermediate_storage,
                        intermediate_sampled: views.intermediate_sampled,
                        total_storage: views.total_storage,
                        selection_weights: views.selection_weights,
                    },
                    &self.compose_indirection,
                    sh,
                    uniform_bind_group_layout,
                    section_id,
                    floor,
                )?;
                match section_id {
                    DIRECT_DELTA_SECTION => promotion_replacement = Some(replacement),
                    ANIMATED_DIRECT_DELTA_SECTION => animated_replacement = Some(replacement),
                    _ => unreachable!("only direct sparse sections reach this branch"),
                }
            }
        }

        // Compute every accounting result before a submission or active
        // binding swap. After this point the transaction consists only of
        // queue encoding, submission, and infallible replacements.
        let current_indirect_active = self.indirect_compose.active_pool_bytes();
        let replacement_indirect_active = indirect_replacement.as_ref().map_or(
            current_indirect_active,
            StreamingIndirectCompose::active_pool_bytes,
        );
        let retiring_indirect_capacity = indirect_replacement
            .as_ref()
            .map(|_| self.indirect_compose.retired_sparse_capacity_bytes())
            .transpose()?;
        let (current_direct_active, current_promotion_active, current_animated_active) =
            self.direct_compose.as_ref().map_or((0, 0, 0), |direct| {
                (
                    direct.active_capacity_bytes(),
                    direct.promotion_active_capacity_bytes(),
                    direct.animated_active_capacity_bytes(),
                )
            });
        let replacement_promotion_active = promotion_replacement.as_ref().map_or(
            current_promotion_active,
            DirectSparseReplacement::active_capacity_bytes,
        );
        let replacement_animated_active = animated_replacement.as_ref().map_or(
            current_animated_active,
            DirectSparseReplacement::active_capacity_bytes,
        );
        let replacement_direct_active = current_direct_active
            .checked_sub(current_promotion_active)
            .and_then(|bytes| bytes.checked_sub(current_animated_active))
            .and_then(|bytes| bytes.checked_add(replacement_promotion_active))
            .and_then(|bytes| bytes.checked_add(replacement_animated_active))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let replacement_active_capacity = self.active_capacity_for(
            self.shape,
            replacement_indirect_active,
            replacement_direct_active,
        )?;
        let (retiring_promotion_capacity, retiring_animated_capacity) = self
            .direct_compose
            .as_ref()
            .map_or(Ok((None, None)), |direct| {
                Ok((
                    promotion_replacement
                        .as_ref()
                        .map(|_| direct.promotion_retired_sparse_capacity_bytes())
                        .transpose()?,
                    animated_replacement
                        .as_ref()
                        .map(|_| direct.animated_retired_sparse_capacity_bytes())
                        .transpose()?,
                ))
            })?;

        // All fallible allocation/validation is complete. Encode retained
        // data into the candidates, submit once, then publish every swap as
        // one infallible CPU transaction.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Streamed SH Sparse Pool Growth Copy"),
        });
        if let Some(replacement) = indirect_replacement.as_ref() {
            self.indirect_compose
                .copy_retained_to(replacement, &mut encoder);
        }
        if promotion_replacement.is_some() || animated_replacement.is_some() {
            let direct = self.direct_compose.as_ref().expect(
                "prevalidated direct sparse replacements require a live direct compose pass",
            );
            if let Some(replacement) = promotion_replacement.as_ref() {
                direct.copy_to_sparse_replacement(replacement, &mut encoder);
            }
            if let Some(replacement) = animated_replacement.as_ref() {
                direct.copy_to_sparse_replacement(replacement, &mut encoder);
            }
        }
        queue.submit(std::iter::once(encoder.finish()));
        let complete = Arc::new(AtomicBool::new(false));
        let callback_complete = Arc::clone(&complete);
        queue.on_submitted_work_done(move || {
            callback_complete.store(true, Ordering::Release);
        });
        if let Some(replacement) = indirect_replacement {
            let compose = std::mem::replace(&mut self.indirect_compose, replacement);
            let resources = compose.into_retired_sparse_resources();
            let capacity_bytes = retiring_indirect_capacity
                .expect("indirect replacement has a precomputed retirement capacity");
            debug_assert_eq!(resources.capacity_bytes(), capacity_bytes);
            self.retiring_indirect = Some(RetiringIndirectGeneration {
                capacity_bytes,
                resources,
                complete: Arc::clone(&complete),
            });
        }
        if let Some(replacement) = promotion_replacement {
            let pass = self
                .direct_compose
                .as_mut()
                .expect("prevalidated id-41 replacement requires direct compose")
                .commit_sparse_replacement(replacement);
            let capacity_bytes = retiring_promotion_capacity
                .expect("id-41 replacement has a precomputed retirement capacity");
            debug_assert_eq!(pass.capacity_bytes(), capacity_bytes);
            self.retiring_direct_promotion = Some(RetiringDirectSparseGeneration {
                capacity_bytes,
                pass,
                complete: Arc::clone(&complete),
            });
        }
        if let Some(replacement) = animated_replacement {
            let pass = self
                .direct_compose
                .as_mut()
                .expect("prevalidated id-45 replacement requires direct compose")
                .commit_sparse_replacement(replacement);
            let capacity_bytes = retiring_animated_capacity
                .expect("id-45 replacement has a precomputed retirement capacity");
            debug_assert_eq!(pass.capacity_bytes(), capacity_bytes);
            self.retiring_direct_animated = Some(RetiringDirectSparseGeneration {
                capacity_bytes,
                pass,
                complete,
            });
        }
        self.sparse_capacity_floors = floors;
        self.active_capacity_bytes = replacement_active_capacity;
        Ok(())
    }

    pub(super) fn release_completed_retirement(&mut self) {
        if self
            .retiring_dense
            .as_ref()
            .is_some_and(|retiring| retiring.complete.load(Ordering::Acquire))
        {
            self.retiring_dense = None;
        }
        if self
            .retiring_indirect
            .as_ref()
            .is_some_and(|retiring| retiring.complete.load(Ordering::Acquire))
        {
            self.retiring_indirect = None;
        }
        if self
            .retiring_direct_promotion
            .as_ref()
            .is_some_and(|retiring| retiring.complete.load(Ordering::Acquire))
        {
            self.retiring_direct_promotion = None;
        }
        if self
            .retiring_direct_animated
            .as_ref()
            .is_some_and(|retiring| retiring.complete.load(Ordering::Acquire))
        {
            self.retiring_direct_animated = None;
        }
    }

    pub(super) fn retiring_capacity_bytes(&self) -> u64 {
        [
            self.retiring_dense
                .as_ref()
                .map(|retiring| retiring.capacity_bytes),
            self.retiring_indirect
                .as_ref()
                .map(|retiring| retiring.capacity_bytes),
            self.retiring_direct_promotion
                .as_ref()
                .map(|retiring| retiring.capacity_bytes),
            self.retiring_direct_animated
                .as_ref()
                .map(|retiring| retiring.capacity_bytes),
        ]
        .into_iter()
        .flatten()
        .try_fold(0u64, u64::checked_add)
        .expect("validated retired streamed pool capacities must not overflow")
    }

    fn active_capacity_for(
        &self,
        shape: AtlasShape,
        indirect_active: u64,
        direct_active: u64,
    ) -> Result<u64, ShResidencyDrainError> {
        let extent = shape.extent();
        let mut bytes = texture_bytes(self.base_format, extent)
            .checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Float, extent))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        if let Some(format) = self.direct_format {
            bytes = bytes
                .checked_add(texture_bytes(format, extent))
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
        }
        if self.direct_total.is_some() {
            bytes = bytes
                .checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Float, extent))
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
        }
        if self.direct_intermediate.is_some() {
            bytes = bytes
                .checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Float, extent))
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
        }
        bytes = bytes
            .checked_add(indirect_active)
            .and_then(|bytes| bytes.checked_add(direct_active))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        Ok(bytes)
    }

    pub(super) const fn probe_occlusion_enabled(&self) -> bool {
        self.probe_occlusion_enabled
    }

    pub(super) const fn effective_floor_bytes(&self) -> u64 {
        self.effective_floor_bytes
    }

    pub(super) fn depth_moment_view(&self) -> wgpu::TextureView {
        self.depth_moments
            .create_view(&wgpu::TextureViewDescriptor {
                label: Some("Streamed SH Depth Moment Shadow View"),
                dimension: Some(wgpu::TextureViewDimension::D3),
                ..Default::default()
            })
    }

    /// Upload only canonical nodes from an id-50 isolated atlas block. Each
    /// scratch is one physical 8×8 cell; no decoded cluster body survives this
    /// call.
    pub(super) fn validate_isolated_tiles(
        &self,
        expected_format: wgpu::TextureFormat,
        block: &[u8],
        local_to_live_slot: &std::collections::BTreeMap<u32, u32>,
    ) -> Result<(), ShResidencyDrainError> {
        if block.len() < 20 {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "isolated atlas upload has no header",
            });
        }
        let format = read_u32(block, 0)?;
        let slots = read_u32(block, 4)?;
        let width = read_u32(block, 8)?;
        let height = read_u32(block, 12)?;
        let layers = read_u32(block, 16)?;
        if texture_format(format)? != expected_format
            || width == 0
            || height == 0
            || layers == 0
            || width % PHYSICAL_TILE_DIMENSION != 0
            || height % PHYSICAL_TILE_DIMENSION != 0
        {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "isolated atlas upload shape or format is invalid",
            });
        }
        let local_tiles_per_row = width / PHYSICAL_TILE_DIMENSION;
        let local_tiles_per_layer = local_tiles_per_row
            .checked_mul(height / PHYSICAL_TILE_DIMENSION)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        if slots
            > local_tiles_per_layer
                .checked_mul(layers)
                .ok_or(ShResidencyDrainError::SlotOverflow)?
        {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "isolated atlas slot count exceeds its layout",
            });
        }
        let payload_bytes = match expected_format {
            wgpu::TextureFormat::Bc6hRgbUfloat => width
                .checked_div(4)
                .and_then(|blocks| blocks.checked_mul(height / 4))
                .and_then(|blocks| blocks.checked_mul(layers))
                .and_then(|blocks| blocks.checked_mul(16))
                .map(u64::from),
            wgpu::TextureFormat::Rgba16Float => width
                .checked_mul(height)
                .and_then(|pixels| pixels.checked_mul(layers))
                .and_then(|pixels| pixels.checked_mul(8))
                .map(u64::from),
            _ => None,
        }
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let actual_payload = u64::try_from(block.len().saturating_sub(20))
            .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        if actual_payload < payload_bytes {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "isolated atlas payload is truncated",
            });
        }
        if local_to_live_slot
            .iter()
            .any(|(&local, &live)| local >= slots || live >= self.shape.slots)
        {
            return Err(ShResidencyDrainError::SlotOverflow);
        }
        Ok(())
    }

    pub(super) fn upload_isolated_tiles(
        &self,
        queue: &wgpu::Queue,
        destination: &wgpu::Texture,
        expected_format: wgpu::TextureFormat,
        block: &[u8],
        local_to_live_slot: &std::collections::BTreeMap<u32, u32>,
    ) -> Result<(), ShResidencyDrainError> {
        self.validate_isolated_tiles(expected_format, block, local_to_live_slot)?;
        let format = read_u32(block, 0)?;
        let slots = read_u32(block, 4)?;
        let width = read_u32(block, 8)?;
        let height = read_u32(block, 12)?;
        let layers = read_u32(block, 16)?;
        debug_assert_eq!(texture_format(format)?, expected_format);
        debug_assert!(width > 0 && height > 0 && layers > 0);
        debug_assert_eq!(width % PHYSICAL_TILE_DIMENSION, 0);
        debug_assert_eq!(height % PHYSICAL_TILE_DIMENSION, 0);
        let local_tiles_per_row = width / PHYSICAL_TILE_DIMENSION;
        let local_tiles_per_layer = local_tiles_per_row
            .checked_mul(height / PHYSICAL_TILE_DIMENSION)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let payload = &block[20..];
        for (&local_slot, &live_slot) in local_to_live_slot {
            debug_assert!(local_slot < slots && live_slot < self.shape.slots);
            let local_layer = local_slot / local_tiles_per_layer;
            let local_in_layer = local_slot % local_tiles_per_layer;
            let local_x = (local_in_layer % local_tiles_per_row) * PHYSICAL_TILE_DIMENSION;
            let local_y = (local_in_layer / local_tiles_per_row) * PHYSICAL_TILE_DIMENSION;
            let target_layer = live_slot / self.shape.tiles_per_layer;
            let target_in_layer = live_slot % self.shape.tiles_per_layer;
            let target_x = (target_in_layer % self.shape.tiles_per_row) * PHYSICAL_TILE_DIMENSION;
            let target_y = (target_in_layer / self.shape.tiles_per_row) * PHYSICAL_TILE_DIMENSION;
            match expected_format {
                wgpu::TextureFormat::Bc6hRgbUfloat => {
                    let blocks_per_row = width / 4;
                    let blocks_per_image = blocks_per_row
                        .checked_mul(height / 4)
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let source_block_x = local_x / 4;
                    let source_block_y = local_y / 4;
                    let image_start = usize::try_from(local_layer)
                        .ok()
                        .and_then(|layer| layer.checked_mul(blocks_per_image as usize))
                        .and_then(|blocks| blocks.checked_mul(16))
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let row_bytes = usize::try_from(blocks_per_row)
                        .ok()
                        .and_then(|blocks| blocks.checked_mul(16))
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let source_start = image_start
                        .checked_add(usize::try_from(source_block_y).unwrap() * row_bytes)
                        .and_then(|offset| {
                            offset.checked_add(usize::try_from(source_block_x).unwrap() * 16)
                        })
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let second_row = source_start
                        .checked_add(row_bytes)
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let mut scratch = [0u8; 64];
                    scratch[..32].copy_from_slice(
                        payload.get(source_start..source_start + 32).ok_or(
                            ShResidencyDrainError::MalformedChunk {
                                cluster_id: 0,
                                reason: "BC6H isolated atlas tile is truncated",
                            },
                        )?,
                    );
                    scratch[32..].copy_from_slice(payload.get(second_row..second_row + 32).ok_or(
                        ShResidencyDrainError::MalformedChunk {
                            cluster_id: 0,
                            reason: "BC6H isolated atlas second row is truncated",
                        },
                    )?);
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: destination,
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: target_x,
                                y: target_y,
                                z: target_layer,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        &scratch,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(32),
                            rows_per_image: Some(2),
                        },
                        wgpu::Extent3d {
                            width: 8,
                            height: 8,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                wgpu::TextureFormat::Rgba16Float => {
                    let image_start = usize::try_from(local_layer)
                        .ok()
                        .and_then(|layer| layer.checked_mul(width as usize))
                        .and_then(|texels| texels.checked_mul(height as usize))
                        .and_then(|texels| texels.checked_mul(8))
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let row_bytes = usize::try_from(width)
                        .ok()
                        .and_then(|pixels| pixels.checked_mul(8))
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let source_start = image_start
                        .checked_add(usize::try_from(local_y).unwrap() * row_bytes)
                        .and_then(|offset| {
                            offset.checked_add(usize::try_from(local_x).unwrap() * 8)
                        })
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let mut scratch = [0u8; 512];
                    for row in 0..8usize {
                        let source = source_start
                            .checked_add(row * row_bytes)
                            .ok_or(ShResidencyDrainError::SlotOverflow)?;
                        scratch[row * 64..(row + 1) * 64].copy_from_slice(
                            payload.get(source..source + 64).ok_or(
                                ShResidencyDrainError::MalformedChunk {
                                    cluster_id: 0,
                                    reason: "RGBA16F isolated atlas tile is truncated",
                                },
                            )?,
                        );
                    }
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: destination,
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: target_x,
                                y: target_y,
                                z: target_layer,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        &scratch,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(64),
                            rows_per_image: Some(8),
                        },
                        wgpu::Extent3d {
                            width: 8,
                            height: 8,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                _ => unreachable!("streamed SH base formats are validated"),
            }
        }
        Ok(())
    }

    pub(super) fn upload_compose_words(&self, queue: &wgpu::Queue, words: &[u32]) {
        let bytes = u32_bytes(words);
        queue.write_buffer(&self.compose_indirection, 0, &bytes);
    }

    pub(super) fn dispatch_indirect_compose<'a>(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        dirty_ranges: &[(u32, u32)],
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'a>>,
    ) -> Result<(), ShResidencyDrainError> {
        self.indirect_compose.dispatch(
            queue,
            encoder,
            uniform_bind_group,
            dirty_ranges,
            timestamp_writes,
        )
    }

    pub(super) fn indirect_has_active_animation(
        &self,
        animation: &crate::render::sh_volume::AnimatedLightBuffers,
    ) -> bool {
        self.indirect_compose.has_active_animation(animation)
    }

    pub(super) fn sparse_entry_capacity(&self, section_id: u32) -> u32 {
        match section_id {
            27 => self.indirect_compose.entry_capacity(),
            DIRECT_DELTA_SECTION | ANIMATED_DIRECT_DELTA_SECTION => self
                .direct_compose
                .as_ref()
                .and_then(|direct| direct.entry_capacity(section_id))
                .unwrap_or(0),
            _ => 0,
        }
    }

    pub(super) fn sparse_tile_f16_capacity(&self, section_id: u32) -> u32 {
        match section_id {
            27 => self.indirect_compose.tile_f16_capacity(),
            DIRECT_DELTA_SECTION | ANIMATED_DIRECT_DELTA_SECTION => self
                .direct_compose
                .as_ref()
                .and_then(|direct| direct.tile_f16_capacity(section_id))
                .unwrap_or(0),
            _ => 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn dispatch_direct_compose<'a>(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        light_term_mask: postretro_render_cpu::frame_uniforms::LightTermMask,
        promotion_override: DirectShDebugOverride,
        animated_override: AnimatedDirectShDebugOverride,
        promoted_animated_states: &[PromotedBakedLightState],
        promotion_ranges: &[(u32, u32)],
        animated_ranges: &[(u32, u32)],
        force_full_resident: bool,
        promotion_timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'a>>,
        animated_timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'a>>,
    ) -> Result<(), ShResidencyDrainError> {
        self.direct_compose
            .as_mut()
            .ok_or(ShResidencyDrainError::GpuCapacity {
                reason: "streamed direct compose dispatch requested without a compose pool",
            })?
            .dispatch(
                queue,
                encoder,
                uniform_bind_group,
                StreamingDirectComposeFrameInputs {
                    light_term_mask,
                    dirty: StreamingDirectDirtyRanges {
                        promotion: promotion_ranges,
                        animated: animated_ranges,
                        force_full_resident,
                    },
                    promotion_override,
                    animated_override,
                    promoted_animated_states,
                    promotion_timestamp_writes,
                    animated_timestamp_writes,
                },
            )
    }

    pub(super) fn upload_indirect_sparse_row(
        &mut self,
        queue: &wgpu::Queue,
        entry_start: u32,
        tile_f16_start: u32,
        row: &super::ParsedSparseRow,
    ) -> Result<(), ShResidencyDrainError> {
        self.indirect_compose
            .upload_sparse_row(queue, entry_start, tile_f16_start, row)
    }

    pub(super) fn validate_indirect_sparse_row(
        &self,
        entry_start: u32,
        tile_f16_start: u32,
        row: &super::ParsedSparseRow,
    ) -> Result<(), ShResidencyDrainError> {
        self.indirect_compose
            .validate_sparse_row(entry_start, tile_f16_start, row)
    }

    pub(super) fn upload_direct_sparse_rows(
        &mut self,
        queue: &wgpu::Queue,
        section_id: u32,
        rows: &[super::direct_compose::DirectSparseRowUpload<'_>],
    ) -> Result<(), ShResidencyDrainError> {
        self.direct_compose
            .as_ref()
            .ok_or(ShResidencyDrainError::GpuCapacity {
                reason: "streamed direct sparse upload requested without a direct compose pool",
            })?
            .upload_sparse_rows(queue, section_id, rows)
    }

    pub(super) fn validate_direct_sparse_rows(
        &self,
        section_id: u32,
        rows: &[super::direct_compose::DirectSparseRowUpload<'_>],
    ) -> Result<(), ShResidencyDrainError> {
        self.direct_compose
            .as_ref()
            .ok_or(ShResidencyDrainError::GpuCapacity {
                reason: "streamed direct sparse validation requested without a direct compose pool",
            })?
            .validate_sparse_rows(section_id, rows)
    }

    pub(super) fn clear_indirect_sparse_row(
        &self,
        queue: &wgpu::Queue,
        row: u32,
    ) -> Result<(), ShResidencyDrainError> {
        self.indirect_compose.clear_row_pair(queue, row)
    }

    pub(super) fn clear_all_indirect_sparse_rows(&self, queue: &wgpu::Queue) {
        self.indirect_compose.clear_all_row_pairs(queue);
    }

    pub(super) fn clear_direct_sparse_row(
        &self,
        queue: &wgpu::Queue,
        section_id: u32,
        row: u32,
    ) -> Result<(), ShResidencyDrainError> {
        self.direct_compose
            .as_ref()
            .ok_or(ShResidencyDrainError::GpuCapacity {
                reason: "streamed direct sparse clear requested without a direct compose pool",
            })?
            .clear_sparse_row_pair(queue, section_id, row)
    }

    pub(super) fn clear_all_direct_sparse_rows(
        &self,
        queue: &wgpu::Queue,
    ) -> Result<(), ShResidencyDrainError> {
        let Some(direct) = self.direct_compose.as_ref() else {
            return Ok(());
        };
        direct.clear_all_row_pairs(queue, DIRECT_DELTA_SECTION)?;
        if self.has_animated_direct_pass {
            direct.clear_all_row_pairs(queue, ANIMATED_DIRECT_DELTA_SECTION)?;
        }
        Ok(())
    }

    pub(super) fn upload_sample_words_and_moments(
        &self,
        queue: &wgpu::Queue,
        words: &[u32],
        updates: &[(u32, u16, u16)],
        grid: [u32; 3],
    ) -> Result<(), ShResidencyDrainError> {
        let bytes = u32_bytes(words);
        queue.write_buffer(&self.sampled_indirection, 0, &bytes);
        let xy = grid[0]
            .checked_mul(grid[1])
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        for &(dense, mean, mean_sq) in updates {
            let x = dense % grid[0];
            let y = (dense / grid[0]) % grid[1];
            let z = dense / xy;
            let word = *words
                .get(dense as usize)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            let pixels = [mean, mean_sq, word as u16, (word >> 16) as u16];
            let mut bytes = [0u8; 8];
            for (index, value) in pixels.into_iter().enumerate() {
                bytes[index * 2..index * 2 + 2].copy_from_slice(&value.to_le_bytes());
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.depth_moments,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z },
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(8),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        Ok(())
    }

    pub(super) fn logical_bytes_per_dense_slot(&self) -> Result<u64, ShResidencyDrainError> {
        let extent = wgpu::Extent3d {
            width: PHYSICAL_TILE_DIMENSION,
            height: PHYSICAL_TILE_DIMENSION,
            depth_or_array_layers: 1,
        };
        // Compose outputs are active physical capacity, not a second logical
        // source payload. Keep this in lockstep with id-50 accounting.
        texture_bytes(self.base_format, extent)
            .checked_add(
                self.direct_format
                    .map_or(0, |format| texture_bytes(format, extent)),
            )
            .ok_or(ShResidencyDrainError::SlotOverflow)
    }
}

pub(super) fn checked_cell_count(dimensions: [u32; 3]) -> Result<u32, ShResidencyDrainError> {
    dimensions.into_iter().try_fold(1u32, |count, dimension| {
        count
            .checked_mul(dimension)
            .ok_or(ShResidencyDrainError::SlotOverflow)
    })
}

fn create_sample_bind_groups(
    device: &wgpu::Device,
    sh: &ShVolumeResources,
    dense: &DenseTextures,
    grid_info: &wgpu::Buffer,
    depth_moments: &wgpu::Texture,
) -> (wgpu::BindGroup, wgpu::BindGroup) {
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Streamed SH Atlas Sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let depth_view = depth_moments.create_view(&wgpu::TextureViewDescriptor {
        label: Some("Streamed SH Depth Moment View"),
        dimension: Some(wgpu::TextureViewDimension::D3),
        ..Default::default()
    });
    let direct_view = dense
        .direct_total_sampled_view
        .as_ref()
        .or(dense.direct_base_view.as_ref())
        .unwrap_or(&sh.direct.atlas_view);
    let entries = vec![
        wgpu::BindGroupEntry {
            binding: BIND_SH_TOTAL_ATLAS,
            resource: wgpu::BindingResource::TextureView(&dense.total_sampled_view),
        },
        wgpu::BindGroupEntry {
            binding: BIND_SH_ATLAS_SAMPLER,
            resource: wgpu::BindingResource::Sampler(&sampler),
        },
        wgpu::BindGroupEntry {
            binding: BIND_SH_GRID_INFO,
            resource: grid_info.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: BIND_ANIM_DESCRIPTORS,
            resource: sh.animation.descriptors.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: BIND_ANIM_SAMPLES,
            resource: sh.animation.anim_samples.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: BIND_SCRIPTED_LIGHT_DESCRIPTORS,
            resource: sh.scripted_light_descriptors.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: BIND_SH_DEPTH_MOMENTS,
            resource: wgpu::BindingResource::TextureView(&depth_view),
        },
        wgpu::BindGroupEntry {
            binding: BIND_SH_DIRECT_ATLAS,
            resource: wgpu::BindingResource::TextureView(direct_view),
        },
        wgpu::BindGroupEntry {
            binding: BIND_BILLBOARD_DIRECT_SCATTER,
            resource: wgpu::BindingResource::TextureView(&sh.billboard_direct_scatter.sampled_view),
        },
    ];
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Streamed SH Volume Bind Group"),
        layout: &sh.bind_group_layout,
        entries: &entries,
    });
    let mut mesh_entries = entries;
    mesh_entries.push(wgpu::BindGroupEntry {
        binding: BIND_DYNAMIC_DIRECT_PARAMS,
        resource: sh.direct.dynamic_direct_params_binding(),
    });
    let mesh_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Streamed SH Mesh Bind Group"),
        layout: &sh.mesh_bind_group_layout,
        entries: &mesh_entries,
    });
    (bind_group, mesh_bind_group)
}

fn create_grid_info(
    device: &wgpu::Device,
    base: &ShStreamBaseMetadata,
    shape: AtlasShape,
    probe_occlusion_enabled: bool,
) -> wgpu::Buffer {
    let extent = shape.extent();
    let grid_bytes = build_grid_info_bytes(ShGridInfoParams {
        grid_origin: base.grid_origin,
        cell_size: base.cell_size,
        grid_dimensions: base.grid_dimensions,
        atlas_dimensions: [extent.width, extent.height],
        tile_dimension: base.tile_dimension,
        tile_border: base.tile_border,
        atlas_tiles_per_row: shape.tiles_per_row,
        physical_tile_stride: STREAMED_SH_PHYSICAL_TILE_STRIDE,
        tiles_per_layer: shape.tiles_per_layer,
        atlas_layer_count: shape.layers,
        present: true,
        probe_occlusion_enabled,
    });
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Streamed SH Grid Info"),
        contents: &grid_bytes,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}

/// Exact fixed bytes created by the streamed-pool constructors before their
/// proportional backing capacities are chosen. Keep this beside those
/// constructors: the floor is a physical allocation contract, not a codec
/// estimate.
pub(super) fn initial_fixed_metadata_bytes(
    base: &ShStreamBaseMetadata,
    sources: &ShStreamSourceMetadata,
    limits: &wgpu::Limits,
    sh: &ShVolumeResources,
) -> Result<u64, ShResidencyDrainError> {
    let indirection_len = u64::try_from(base.probes.len())
        .map_err(|_| ShResidencyDrainError::SlotOverflow)?
        .checked_mul(4)
        .ok_or(ShResidencyDrainError::SlotOverflow)?
        .max(4);
    let depth_extent = wgpu::Extent3d {
        width: base.grid_dimensions[0].max(1),
        height: base.grid_dimensions[1].max(1),
        depth_or_array_layers: base.grid_dimensions[2].max(1),
    };
    let mut bytes = indirection_len
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(96))
        .and_then(|bytes| {
            bytes.checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Uint, depth_extent))
        })
        .and_then(|bytes| bytes.checked_add(sh.animation.descriptors.size()))
        .and_then(|bytes| bytes.checked_add(sh.animation.anim_samples.size()))
        .and_then(|bytes| bytes.checked_add(sh.scripted_light_descriptors.size()))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    bytes = bytes
        .checked_add(indirect_fixed_metadata_bytes(
            base,
            sources.indirect_delta.as_ref(),
            limits,
        )?)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    if sources.direct_delta.is_some() || sources.animated_direct_delta.is_some() {
        bytes = bytes
            .checked_add(direct_pass_fixed_metadata_bytes(
                base,
                sources.direct_delta.as_ref(),
                limits,
                48,
            )?)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        if let Some(source) = sources.animated_direct_delta.as_ref() {
            let descriptor_bytes = u64::try_from(source.animation_descriptor_indices.len().max(1))
                .map_err(|_| ShResidencyDrainError::SlotOverflow)?
                .checked_mul(4)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            bytes = bytes
                .checked_add(direct_pass_fixed_metadata_bytes(
                    base,
                    Some(source),
                    limits,
                    1_040,
                )?)
                .and_then(|bytes| bytes.checked_add(descriptor_bytes))
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
        }
    }
    Ok(bytes)
}

/// Validate every storage/uniform carrier that the streamed constructors will
/// create, including the dummy bindings required when a sparse family is
/// absent. This runs before the first texture or buffer allocation so an
/// adapter-limit failure cannot leave a partially installed streamed level.
fn preflight_initial_resource_limits(
    base: &ShStreamBaseMetadata,
    sources: &ShStreamSourceMetadata,
    shape: AtlasShape,
    sparse_capacities: &SparseCapacityFloors,
    limits: &wgpu::Limits,
) -> Result<(), ShResidencyDrainError> {
    let indirection_len = u64::try_from(base.probes.len())
        .map_err(|_| ShResidencyDrainError::SlotOverflow)?
        .checked_mul(4)
        .ok_or(ShResidencyDrainError::SlotOverflow)?
        .max(4);
    validate_storage_limits(
        indirection_len,
        limits,
        "streamed SH compose indirection exceeds adapter storage limits",
    )?;
    validate_storage_limits(
        indirection_len,
        limits,
        "streamed SH sampled indirection exceeds adapter storage limits",
    )?;
    let depth_dimensions = base.grid_dimensions.map(|axis| axis.max(1));
    if depth_dimensions
        .into_iter()
        .any(|axis| axis > limits.max_texture_dimension_3d)
    {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed SH depth-moment grid exceeds adapter 3D texture limits",
        });
    }
    preflight_sparse_carrier(
        base,
        sources.indirect_delta.as_ref(),
        sparse_capacities.get(&27).copied(),
        limits,
        true,
    )?;
    if sources.direct_delta.is_some() || sources.animated_direct_delta.is_some() {
        preflight_sparse_carrier(
            base,
            sources.direct_delta.as_ref(),
            sparse_capacities.get(&DIRECT_DELTA_SECTION).copied(),
            limits,
            false,
        )?;
        if let Some(animated) = sources.animated_direct_delta.as_ref() {
            preflight_sparse_carrier(
                base,
                Some(animated),
                sparse_capacities
                    .get(&ANIMATED_DIRECT_DELTA_SECTION)
                    .copied(),
                limits,
                true,
            )?;
        }
    }
    let extent = shape.extent();
    if extent.width > limits.max_texture_dimension_2d
        || extent.height > limits.max_texture_dimension_2d
        || extent.depth_or_array_layers > limits.max_texture_array_layers
    {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed dense atlas exceeds adapter texture limits",
        });
    }
    Ok(())
}

fn preflight_sparse_carrier(
    base: &ShStreamBaseMetadata,
    source: Option<&postretro_level_loader::ShStreamSparseMetadata>,
    sparse_floor: Option<(u32, u32)>,
    limits: &wgpu::Limits,
    has_descriptor_indices: bool,
) -> Result<(), ShResidencyDrainError> {
    let (affinity_dims, _masks, _levels, descriptors, entries, tiles) =
        sparse_compose_capacity(base, source, sparse_floor)?;
    let rows = checked_cell_count(affinity_dims)?;
    let grid_bytes = dynamic_grid_bytes(rows, limits)?;
    validate_uniform_limits(
        grid_bytes,
        limits,
        "streamed dirty compose records exceed adapter buffer limit",
    )?;
    let row_pairs = u64::from(rows.max(1))
        .checked_mul(8)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let entry_bytes = u64::from(entries)
        .checked_mul(4)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let tile_bytes = u64::from(tiles.div_ceil(2))
        .checked_mul(4)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let compaction_bytes = u64::from(rows)
        .checked_mul(3)
        .and_then(|words| words.checked_add(u64::from(entries)))
        .and_then(|words| words.checked_mul(4))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    for (bytes, reason) in [
        (
            row_pairs,
            "streamed sparse CSR row-pair table exceeds adapter storage limits",
        ),
        (
            entry_bytes,
            "streamed sparse entry pool exceeds adapter storage limits",
        ),
        (
            tile_bytes,
            "streamed sparse delta tile pool exceeds adapter storage limits",
        ),
        (
            compaction_bytes,
            "streamed sparse compaction table exceeds adapter storage limits",
        ),
    ] {
        validate_storage_limits(bytes, limits, reason)?;
    }
    if has_descriptor_indices {
        let descriptor_bytes = u64::try_from(descriptors.len().max(1))
            .map_err(|_| ShResidencyDrainError::SlotOverflow)?
            .checked_mul(4)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        validate_storage_limits(
            descriptor_bytes,
            limits,
            "streamed sparse descriptor indices exceed adapter storage limits",
        )?;
    }
    Ok(())
}

fn validate_storage_limits(
    bytes: u64,
    limits: &wgpu::Limits,
    reason: &'static str,
) -> Result<(), ShResidencyDrainError> {
    if bytes > limits.max_buffer_size || bytes > u64::from(limits.max_storage_buffer_binding_size) {
        return Err(ShResidencyDrainError::GpuCapacity { reason });
    }
    Ok(())
}

fn validate_uniform_limits(
    bytes: u64,
    limits: &wgpu::Limits,
    reason: &'static str,
) -> Result<(), ShResidencyDrainError> {
    if bytes > limits.max_buffer_size {
        return Err(ShResidencyDrainError::GpuCapacity { reason });
    }
    Ok(())
}

fn indirect_fixed_metadata_bytes(
    base: &ShStreamBaseMetadata,
    source: Option<&postretro_level_loader::ShStreamSparseMetadata>,
    limits: &wgpu::Limits,
) -> Result<u64, ShResidencyDrainError> {
    let (rows, descriptor_count, source_present) = sparse_fixed_shape(base, source)?;
    let grid = dynamic_grid_bytes(rows, limits)?;
    let pairs = u64::from(rows.max(1))
        .checked_mul(8)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let compaction_prefix = u64::from(rows)
        .checked_mul(3)
        .and_then(|words| words.checked_mul(4))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let descriptors = u64::try_from(descriptor_count.max(1))
        .map_err(|_| ShResidencyDrainError::SlotOverflow)?
        .checked_mul(4)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let dummy_backing = (!source_present).then_some(12).unwrap_or(0);
    grid.checked_add(pairs)
        .and_then(|bytes| bytes.checked_add(compaction_prefix))
        .and_then(|bytes| bytes.checked_add(32))
        .and_then(|bytes| bytes.checked_add(descriptors))
        .and_then(|bytes| bytes.checked_add(dummy_backing))
        .ok_or(ShResidencyDrainError::SlotOverflow)
}

fn direct_pass_fixed_metadata_bytes(
    base: &ShStreamBaseMetadata,
    source: Option<&postretro_level_loader::ShStreamSparseMetadata>,
    limits: &wgpu::Limits,
    pass_uniform_bytes: u64,
) -> Result<u64, ShResidencyDrainError> {
    let (rows, _, source_present) = sparse_fixed_shape(base, source)?;
    let grid = dynamic_grid_bytes(rows, limits)?;
    let pairs = u64::from(rows.max(1))
        .checked_mul(8)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let compaction_prefix = u64::from(rows)
        .checked_mul(3)
        .and_then(|words| words.checked_mul(4))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let dummy_backing = (!source_present).then_some(12).unwrap_or(0);
    grid.checked_add(pairs)
        .and_then(|bytes| bytes.checked_add(compaction_prefix))
        .and_then(|bytes| bytes.checked_add(pass_uniform_bytes))
        .and_then(|bytes| bytes.checked_add(dummy_backing))
        .ok_or(ShResidencyDrainError::SlotOverflow)
}

fn sparse_fixed_shape(
    base: &ShStreamBaseMetadata,
    source: Option<&postretro_level_loader::ShStreamSparseMetadata>,
) -> Result<(u32, usize, bool), ShResidencyDrainError> {
    match source {
        Some(source) => Ok((
            checked_cell_count(source.affinity_dims)?,
            source.animation_descriptor_indices.len(),
            true,
        )),
        None => Ok((
            checked_cell_count(base.grid_dimensions.map(|axis| axis.div_ceil(4)))?,
            0,
            false,
        )),
    }
}

fn dynamic_grid_bytes(rows: u32, limits: &wgpu::Limits) -> Result<u64, ShResidencyDrainError> {
    let record = u64::try_from(DYNAMIC_COMPOSE_GRID_DIMS_SIZE)
        .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
    let alignment = u64::from(limits.min_uniform_buffer_offset_alignment.max(1));
    let stride = record
        .checked_add(alignment - 1)
        .and_then(|bytes| bytes.checked_div(alignment))
        .and_then(|records| records.checked_mul(alignment))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let bytes = stride
        .checked_mul(u64::from(rows.max(1)))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    if bytes > limits.max_buffer_size {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed dirty compose records exceed adapter buffer limit",
        });
    }
    Ok(bytes)
}

pub(super) fn sparse_compose_capacity(
    base: &ShStreamBaseMetadata,
    source: Option<&postretro_level_loader::ShStreamSparseMetadata>,
    sparse_floor: Option<(u32, u32)>,
) -> Result<([u32; 3], Vec<u64>, Vec<u8>, Vec<u32>, u32, u32), ShResidencyDrainError> {
    if let Some(source) = source {
        let cells = checked_cell_count(source.affinity_dims)?;
        let cells_usize =
            usize::try_from(cells).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        if source.valid_probe_masks.len() != cells_usize
            || source.cell_levels.len() != cells_usize
            || source.affinity_offsets.len() != cells_usize.saturating_add(1)
        {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed indirect sparse metadata has inconsistent row tables",
            });
        }
        let entries = *source.affinity_offsets.last().unwrap_or(&0);
        if usize::try_from(entries).ok() != Some(source.affinity_lights.len()) {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed indirect sparse metadata has inconsistent entry table",
            });
        }
        if source
            .affinity_offsets
            .windows(2)
            .any(|pair| pair[0] > pair[1])
        {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed indirect sparse metadata offsets are not monotonic",
            });
        }
        let stride = u32::try_from(delta_probe_f16_stride(source.tile_dimension))
            .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        let mut max_row_entries = 0u32;
        let mut max_row_tile_f16 = 0u32;
        for row in 0..cells_usize {
            let level = Level::from_u8(source.cell_levels[row]).ok_or(
                ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "streamed indirect sparse metadata has invalid cell level",
                },
            )?;
            let tiles = u32::try_from(stored_delta_tiles(level, source.valid_probe_masks[row]))
                .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
            let row_entries = source.affinity_offsets[row + 1] - source.affinity_offsets[row];
            let row_tiles = tiles
                .checked_mul(stride)
                .and_then(|count| count.checked_mul(row_entries))
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            max_row_entries = max_row_entries.max(row_entries);
            max_row_tile_f16 = max_row_tile_f16.max(row_tiles);
        }
        return Ok((
            source.affinity_dims,
            source.valid_probe_masks.clone(),
            source.cell_levels.clone(),
            source.animation_descriptor_indices.clone(),
            // One largest canonical row is sufficient for the initial sparse
            // floor. Pool growth adds backing rather than retaining every
            // legacy CSR payload at level installation.
            max_row_entries
                .checked_add(1)
                .ok_or(ShResidencyDrainError::SlotOverflow)?
                .max(sparse_floor.map_or(1, |floor| floor.0)),
            max_row_tile_f16
                .checked_add(max_row_tile_f16 & 1)
                .and_then(|count| count.checked_add(2))
                .ok_or(ShResidencyDrainError::SlotOverflow)?
                .max(sparse_floor.map_or(2, |floor| floor.1)),
        ));
    }

    // Base-only streamed maps still use the indirect compose shader for the
    // id-34 → total copy. Derive its cell metadata from the retained id-34
    // projection instead of requiring a dummy id-27 body.
    let dims = base.grid_dimensions.map(|axis| axis.div_ceil(4));
    let cells = checked_cell_count(dims)?;
    let cells_usize = usize::try_from(cells).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
    let mut masks = vec![0u64; cells_usize];
    let mut levels = vec![0u8; cells_usize];
    let [width, height, _] = base.grid_dimensions;
    let xy = width
        .checked_mul(height)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    for (dense, probe) in base.probes.iter().enumerate() {
        let dense = u32::try_from(dense).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        let xyz = [dense % width, (dense / width) % height, dense / xy];
        let cell_xyz = xyz.map(|axis| axis / 4);
        let cell = cell_xyz[0]
            .checked_add(
                cell_xyz[1]
                    .checked_mul(dims[0])
                    .ok_or(ShResidencyDrainError::SlotOverflow)?,
            )
            .and_then(|index| {
                index.checked_add(cell_xyz[2].checked_mul(dims[0].checked_mul(dims[1])?)?)
            })
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let local = (xyz[0] % 4) + (xyz[1] % 4) * 4 + (xyz[2] % 4) * 16;
        let cell_index = usize::try_from(cell).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        if probe.validity != 0 {
            masks[cell_index] |= 1u64 << local;
        }
        levels[cell_index] = probe.density_level;
    }
    // Preserve the packed-u32 sentinel even when this base-only path never
    // allocates delta row payloads. It keeps the CPU and GPU sparse pools on
    // the same word-aligned address contract.
    Ok((
        dims,
        masks,
        levels,
        vec![u32::MAX],
        sparse_floor.map_or(1, |floor| floor.0).max(1),
        sparse_floor.map_or(2, |floor| floor.1).max(2),
    ))
}

pub(super) fn compose_origin_bytes(origin: [f32; 3], cell_size: [f32; 3]) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    for (index, value) in origin.into_iter().enumerate() {
        bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (index, value) in cell_size.into_iter().enumerate() {
        let offset = 16 + index * 4;
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

pub(super) fn buffer_with_zeroes(
    device: &wgpu::Device,
    label: &'static str,
    byte_len: usize,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: &vec![0; byte_len.max(4)],
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
    })
}

pub(super) fn validate_storage_buffer_size(
    device: &wgpu::Device,
    byte_len: u64,
    reason: &'static str,
) -> Result<(), ShResidencyDrainError> {
    let limits = device.limits();
    if byte_len > limits.max_buffer_size
        || byte_len > u64::from(limits.max_storage_buffer_binding_size)
    {
        return Err(ShResidencyDrainError::GpuCapacity { reason });
    }
    Ok(())
}

fn compose_bgl_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let storage = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
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
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
            count: None,
        },
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
        storage(BIND_DELTA_SUBBLOCKS),
        storage(BIND_AFFINITY_OFFSETS),
        storage(22),
        storage(23),
        storage(BIND_AFFINITY_LIGHTS),
        storage(BIND_ANIMATION_DESCRIPTOR_INDICES),
        storage(BIND_PROBE_INDIRECTION),
        storage(BIND_DELTA_COMPACTION_META),
    ]
}

pub(super) fn texture_format(format: u32) -> Result<wgpu::TextureFormat, ShResidencyDrainError> {
    match format {
        IRRADIANCE_FORMAT_BC6H => Ok(wgpu::TextureFormat::Bc6hRgbUfloat),
        IRRADIANCE_FORMAT_RGBA16F => Ok(wgpu::TextureFormat::Rgba16Float),
        _ => Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed SH metadata has an unsupported atlas format",
        }),
    }
}

pub(super) fn array_view(texture: &wgpu::Texture, label: &'static str) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some(label),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

fn texture_bytes(format: wgpu::TextureFormat, extent: wgpu::Extent3d) -> u64 {
    match format {
        wgpu::TextureFormat::Bc6hRgbUfloat => {
            u64::from(extent.width.div_ceil(4))
                * u64::from(extent.height.div_ceil(4))
                * u64::from(extent.depth_or_array_layers)
                * 16
        }
        wgpu::TextureFormat::Rgba16Float | wgpu::TextureFormat::Rgba16Uint => {
            u64::from(extent.width)
                * u64::from(extent.height)
                * u64::from(extent.depth_or_array_layers)
                * 8
        }
        _ => 0,
    }
}

fn copy_texture(
    encoder: &mut wgpu::CommandEncoder,
    source: &wgpu::Texture,
    destination: &wgpu::Texture,
    extent: wgpu::Extent3d,
) {
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: source,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: destination,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        extent,
    );
}

fn copy_buffer(
    encoder: &mut wgpu::CommandEncoder,
    source: &wgpu::Buffer,
    destination: &wgpu::Buffer,
) {
    let size = source.size().min(destination.size());
    if size != 0 {
        encoder.copy_buffer_to_buffer(source, 0, destination, 0, size);
    }
}

/// Sparse growth has one active and at most one retiring generation per
/// family. A retirement in one CSR family must never block growth in either
/// sibling family: their tickets retain only their own four backing buffers.
fn sparse_family_has_retiring_generation(
    section_id: u32,
    indirect_retiring: bool,
    promotion_retiring: bool,
    animated_retiring: bool,
) -> bool {
    match section_id {
        27 => indirect_retiring,
        DIRECT_DELTA_SECTION => promotion_retiring,
        ANIMATED_DIRECT_DELTA_SECTION => animated_retiring,
        _ => true,
    }
}

fn integer_sqrt_ceil(value: u32) -> u32 {
    let root = (value as f64).sqrt() as u32;
    if root.saturating_mul(root) == value {
        root
    } else {
        root + 1
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ShResidencyDrainError> {
    bytes
        .get(offset..offset + 4)
        .map(|word| u32::from_le_bytes(word.try_into().expect("four-byte slice")))
        .ok_or(ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "streamed GPU upload reads a truncated u32",
        })
}

pub(super) fn u32_bytes(words: &[u32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(words.len() * std::mem::size_of::<u32>());
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes
}

fn u16_words(halves: &[u16]) -> Result<Vec<u32>, ShResidencyDrainError> {
    Ok(halves
        .chunks(2)
        .map(|pair| u32::from(pair[0]) | (u32::from(pair.get(1).copied().unwrap_or(0)) << 16))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_retirement_pressure_is_scoped_to_the_replaced_family() {
        // An in-flight id-27 copy blocks only a second id-27 replacement;
        // independent id-41/id-45 backing pools can still grow.
        assert!(sparse_family_has_retiring_generation(
            27, true, false, false
        ));
        assert!(!sparse_family_has_retiring_generation(
            DIRECT_DELTA_SECTION,
            true,
            false,
            false,
        ));
        assert!(!sparse_family_has_retiring_generation(
            ANIMATED_DIRECT_DELTA_SECTION,
            true,
            false,
            false,
        ));

        // The symmetric cases protect the coupled dense generation from
        // hidden references inside sparse retirement tickets: each direct
        // family receives its own independent wait slot.
        assert!(!sparse_family_has_retiring_generation(
            27, false, true, true
        ));
        assert!(sparse_family_has_retiring_generation(
            DIRECT_DELTA_SECTION,
            false,
            true,
            false,
        ));
        assert!(sparse_family_has_retiring_generation(
            ANIMATED_DIRECT_DELTA_SECTION,
            false,
            false,
            true,
        ));
    }
}
