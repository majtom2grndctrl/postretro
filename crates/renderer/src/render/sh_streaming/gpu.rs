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

mod bindings;
mod capacity;
mod growth;
mod indirect;
mod setup;
mod upload;

use bindings::{create_grid_info, create_sample_bind_groups};
use capacity::preflight_initial_resource_limits;
pub(in crate::render::sh_streaming) use capacity::{
    initial_fixed_metadata_bytes, sparse_compose_capacity,
};

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
    pub(super) dense_group_minimum_bytes: u64,
    pub(super) sparse_group_minimum_bytes: std::collections::BTreeMap<u32, u64>,
    probe_occlusion_enabled: bool,
    sparse_capacity_floors: SparseCapacityFloors,
}

pub(super) fn checked_cell_count(dimensions: [u32; 3]) -> Result<u32, ShResidencyDrainError> {
    dimensions.into_iter().try_fold(1u32, |count, dimension| {
        count
            .checked_mul(dimension)
            .ok_or(ShResidencyDrainError::SlotOverflow)
    })
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
