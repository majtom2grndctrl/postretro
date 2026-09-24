// SH allocation descriptions shared by level-install resource owners.
// See: context/lib/rendering_pipeline.md §4, §7.1

use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_render_cpu::sh_compose::{pad_storage_bytes, u16_slice_to_bytes, u32_slice_to_bytes};
use postretro_render_cpu::sh_volume::{ANIMATION_DESCRIPTOR_SIZE, SCRIPTED_FLOATS_PER_LIGHT};

/// Named physical allocations that make up level-owned SH residency.
///
/// This is deliberately only a description vocabulary. Task 5 records these
/// descriptions; keeping it free of reporting means this behavior-preserving
/// extraction does not add measurement-mode work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShAllocationKind {
    IndirectBaseAtlas,
    IndirectTotalAtlas,
    DepthMoments,
    GridInfo,
    AnimatedLightDescriptors,
    AnimatedLightSamples,
    ScriptedLightDescriptors,
    DirectBaseAtlas,
    DirectDynamicParams,
    DirectComposedAtlas,
    DirectIntermediateAtlas,
    IndirectComposeDeltaSubblocks,
    IndirectComposeAffinityOffsets,
    IndirectComposeAffinityLights,
    IndirectComposeDescriptorIndices,
    IndirectComposeProbeIndirection,
    IndirectComposeCompactionMetadata,
    IndirectComposeGrid,
    IndirectComposeOrigin,
    DirectComposeDeltaSubblocks,
    DirectComposeCompactionMetadata,
    DirectComposeAffinityOffsets,
    DirectComposeAffinityLights,
    DirectComposeProbeIndirection,
    DirectComposeGrid,
    DirectComposeDebugOverride,
    DirectComposeLightTermMask,
    AnimatedDirectComposeDeltaSubblocks,
    AnimatedDirectComposeCompactionMetadata,
    AnimatedDirectComposeAffinityOffsets,
    AnimatedDirectComposeAffinityLights,
    AnimatedDirectComposeDescriptorIndices,
    AnimatedDirectComposeProbeIndirection,
    AnimatedDirectComposeGrid,
    AnimatedDirectComposeLightScale,
    BillboardDirectScatterBaseVolume,
    BillboardDirectScatterComposedVolume,
    BillboardComposeGrid,
    BillboardComposeDeltas,
    BillboardComposeOffsets,
    BillboardComposeLights,
    BillboardComposeDescriptorIndices,
}

/// The exact texture request sent to wgpu for one physical SH texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TextureAllocation {
    pub(super) kind: ShAllocationKind,
    pub(super) format: wgpu::TextureFormat,
    pub(super) extent: wgpu::Extent3d,
    pub(super) dimension: wgpu::TextureDimension,
    pub(super) usage: wgpu::TextureUsages,
}

impl TextureAllocation {
    pub(super) fn descriptor<'a>(&self, label: Option<&'a str>) -> wgpu::TextureDescriptor<'a> {
        wgpu::TextureDescriptor {
            label,
            size: self.extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: self.dimension,
            format: self.format,
            usage: self.usage,
            view_formats: &[],
        }
    }
}

/// The exact byte count and usage request for one physical SH buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BufferAllocation {
    pub(super) kind: ShAllocationKind,
    pub(super) byte_len: usize,
    pub(super) usage: wgpu::BufferUsages,
}

/// Bytes prepared for a storage binding together with the allocation it backs.
pub(super) struct StorageBufferPayload {
    pub(super) allocation: BufferAllocation,
    pub(super) contents: Vec<u8>,
}

impl StorageBufferPayload {
    fn new(kind: ShAllocationKind, bytes: Vec<u8>, empty_minimum: usize) -> Self {
        let contents = pad_storage_bytes(bytes, empty_minimum);
        let allocation = buffer_allocation(kind, &contents, wgpu::BufferUsages::STORAGE);
        Self {
            allocation,
            contents,
        }
    }
}

pub(super) fn buffer_allocation(
    kind: ShAllocationKind,
    contents: &[u8],
    usage: wgpu::BufferUsages,
) -> BufferAllocation {
    BufferAllocation {
        kind,
        byte_len: contents.len(),
        usage,
    }
}

pub(super) fn scripted_light_sample_reserve_bytes(capacity: usize) -> usize {
    capacity * SCRIPTED_FLOATS_PER_LIGHT * size_of::<f32>()
}

pub(super) fn scripted_light_descriptor_bytes(capacity: usize) -> Vec<u8> {
    vec![0; capacity.max(1) * ANIMATION_DESCRIPTOR_SIZE]
}

/// Requested texture bytes for the formats used by level-owned SH resources.
/// Compressed sizes use the physical 4×4 block footprint, not logical texels.
pub(super) fn texture_allocation_bytes(allocation: TextureAllocation) -> u64 {
    let extent = allocation.extent;
    match allocation.format {
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
        format => panic!("unexpected SH allocation format {format:?}"),
    }
}

fn texture_allocation(
    kind: ShAllocationKind,
    format: wgpu::TextureFormat,
    extent: wgpu::Extent3d,
    dimension: wgpu::TextureDimension,
    usage: wgpu::TextureUsages,
) -> TextureAllocation {
    TextureAllocation {
        kind,
        format,
        extent,
        dimension,
        usage,
    }
}

fn sampled_upload_usage() -> wgpu::TextureUsages {
    wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST
}

pub(super) fn indirect_base_atlas_allocation(
    section: Option<&OctahedralShVolumeSection>,
) -> TextureAllocation {
    let Some(section) = section else {
        return indirect_base_atlas_dummy_allocation();
    };

    let empty = section.atlas_dimensions[0] == 0
        || section.atlas_dimensions[1] == 0
        || section.layer_count == 0;
    let (format, width, height, layers) = match section.irradiance_format {
        IRRADIANCE_FORMAT_BC6H if empty => (wgpu::TextureFormat::Bc6hRgbUfloat, 4, 4, 1),
        IRRADIANCE_FORMAT_BC6H => (
            wgpu::TextureFormat::Bc6hRgbUfloat,
            section.atlas_dimensions[0].div_ceil(4) * 4,
            section.atlas_dimensions[1].div_ceil(4) * 4,
            section.layer_count,
        ),
        IRRADIANCE_FORMAT_RGBA16F if empty => (wgpu::TextureFormat::Rgba16Float, 1, 1, 1),
        IRRADIANCE_FORMAT_RGBA16F => (
            wgpu::TextureFormat::Rgba16Float,
            section.atlas_dimensions[0],
            section.atlas_dimensions[1],
            section.layer_count,
        ),
        // The PRL parser rejects unknown tags. Keep manually constructed test
        // sections from silently selecting an upload format.
        unknown => panic!("unsupported compact SH irradiance format tag {unknown}"),
    };
    texture_allocation(
        ShAllocationKind::IndirectBaseAtlas,
        format,
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: layers,
        },
        wgpu::TextureDimension::D2,
        sampled_upload_usage(),
    )
}

pub(super) fn indirect_base_atlas_dummy_allocation() -> TextureAllocation {
    texture_allocation(
        ShAllocationKind::IndirectBaseAtlas,
        wgpu::TextureFormat::Rgba16Float,
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        wgpu::TextureDimension::D2,
        sampled_upload_usage(),
    )
}

pub(super) fn indirect_base_atlas_empty_payload(section: &OctahedralShVolumeSection) -> bool {
    section.atlas_dimensions[0] == 0 || section.atlas_dimensions[1] == 0 || section.layer_count == 0
}

pub(super) fn indirect_total_atlas_allocation(
    atlas_dimensions: [u32; 2],
    layer_count: u32,
    include_copy_src: bool,
) -> TextureAllocation {
    let mut usage = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING;
    if include_copy_src {
        usage |= wgpu::TextureUsages::COPY_SRC;
    }
    texture_allocation(
        ShAllocationKind::IndirectTotalAtlas,
        wgpu::TextureFormat::Rgba16Float,
        wgpu::Extent3d {
            width: atlas_dimensions[0].max(1),
            height: atlas_dimensions[1].max(1),
            depth_or_array_layers: layer_count.max(1),
        },
        wgpu::TextureDimension::D2,
        usage,
    )
}

pub(super) fn depth_moment_allocation(grid_dimensions: [u32; 3]) -> TextureAllocation {
    texture_allocation(
        ShAllocationKind::DepthMoments,
        wgpu::TextureFormat::Rgba16Uint,
        wgpu::Extent3d {
            width: grid_dimensions[0].max(1),
            height: grid_dimensions[1].max(1),
            depth_or_array_layers: grid_dimensions[2].max(1),
        },
        wgpu::TextureDimension::D3,
        sampled_upload_usage(),
    )
}

pub(super) fn direct_base_atlas_allocation(section: &DirectShVolumeSection) -> TextureAllocation {
    let (format, width, height) = if section.irradiance_format == IRRADIANCE_FORMAT_BC6H {
        (
            wgpu::TextureFormat::Bc6hRgbUfloat,
            section.atlas_dimensions[0].div_ceil(4) * 4,
            section.atlas_dimensions[1].div_ceil(4) * 4,
        )
    } else {
        (
            wgpu::TextureFormat::Rgba16Float,
            section.atlas_dimensions[0],
            section.atlas_dimensions[1],
        )
    };
    texture_allocation(
        ShAllocationKind::DirectBaseAtlas,
        format,
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: section.layer_count,
        },
        wgpu::TextureDimension::D2,
        sampled_upload_usage(),
    )
}

pub(super) fn direct_base_atlas_dummy_allocation() -> TextureAllocation {
    texture_allocation(
        ShAllocationKind::DirectBaseAtlas,
        wgpu::TextureFormat::Bc6hRgbUfloat,
        wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
        wgpu::TextureDimension::D2,
        sampled_upload_usage(),
    )
}

pub(super) fn direct_composed_atlas_allocation(
    kind: ShAllocationKind,
    atlas_dimensions: [u32; 2],
    layer_count: u32,
) -> TextureAllocation {
    debug_assert!(matches!(
        kind,
        ShAllocationKind::DirectComposedAtlas | ShAllocationKind::DirectIntermediateAtlas
    ));
    texture_allocation(
        kind,
        wgpu::TextureFormat::Rgba16Float,
        wgpu::Extent3d {
            width: atlas_dimensions[0].max(1),
            height: atlas_dimensions[1].max(1),
            depth_or_array_layers: layer_count.max(1),
        },
        wgpu::TextureDimension::D2,
        wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
    )
}

pub(super) fn billboard_scatter_base_allocation(grid_dimensions: [u32; 3]) -> TextureAllocation {
    billboard_scatter_volume_allocation(
        ShAllocationKind::BillboardDirectScatterBaseVolume,
        grid_dimensions,
        sampled_upload_usage(),
    )
}

pub(super) fn billboard_scatter_dummy_allocation() -> TextureAllocation {
    billboard_scatter_base_allocation([1, 1, 1])
}

pub(super) fn billboard_scatter_composed_allocation(
    grid_dimensions: [u32; 3],
) -> TextureAllocation {
    billboard_scatter_volume_allocation(
        ShAllocationKind::BillboardDirectScatterComposedVolume,
        grid_dimensions,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
    )
}

fn billboard_scatter_volume_allocation(
    kind: ShAllocationKind,
    grid_dimensions: [u32; 3],
    usage: wgpu::TextureUsages,
) -> TextureAllocation {
    texture_allocation(
        kind,
        wgpu::TextureFormat::Rgba16Float,
        wgpu::Extent3d {
            width: grid_dimensions[0],
            height: grid_dimensions[1],
            depth_or_array_layers: grid_dimensions[2],
        },
        wgpu::TextureDimension::D3,
        usage,
    )
}

/// Stored-tile geometry shared by direct-SH promotion and animated-add passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DirectAtlasLayout {
    pub(super) grid_dimensions: [u32; 3],
    pub(super) atlas_dimensions: [u32; 2],
    pub(super) tile_dimension: u32,
    pub(super) tile_border: u32,
    pub(super) atlas_tiles_per_row: u32,
    pub(super) tiles_per_layer: u32,
    pub(super) atlas_layer_count: u32,
}

impl DirectAtlasLayout {
    pub(super) fn from_direct_section(section: &DirectShVolumeSection) -> Self {
        Self {
            grid_dimensions: section.grid_dimensions,
            atlas_dimensions: section.atlas_dimensions,
            tile_dimension: section.tile_dimension,
            tile_border: section.tile_border,
            atlas_tiles_per_row: section.atlas_tiles_per_row,
            tiles_per_layer: section.tiles_per_layer,
            atlas_layer_count: section.layer_count,
        }
    }

    pub(super) fn from_sh_section(section: &OctahedralShVolumeSection) -> Self {
        Self {
            grid_dimensions: section.grid_dimensions,
            atlas_dimensions: section.atlas_dimensions,
            tile_dimension: section.tile_dimension,
            tile_border: section.tile_border,
            atlas_tiles_per_row: section.atlas_tiles_per_row,
            tiles_per_layer: section.tiles_per_layer,
            atlas_layer_count: section.layer_count,
        }
    }
}

/// The direct-SH compose texture decision. The resource owner still gates the
/// request on the actual uploaded-base result, so device-limit fallback keeps
/// binding the direct dummy instead of allocating a compose target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct DirectAtlasUsage {
    pub(super) needs_composed_atlas: bool,
    pub(super) needs_intermediate_atlas: bool,
    pub(super) atlas_dimensions: [u32; 2],
    pub(super) layer_count: u32,
    layout: Option<DirectAtlasLayout>,
}

impl DirectAtlasUsage {
    pub(super) fn layout(self) -> Option<DirectAtlasLayout> {
        self.layout
    }
}

pub(super) fn direct_atlas_usage(
    direct_section: Option<&DirectShVolumeSection>,
    has_animated_direct: bool,
    fallback_layout: Option<DirectAtlasLayout>,
) -> DirectAtlasUsage {
    if direct_section.is_none() && !has_animated_direct {
        return DirectAtlasUsage::default();
    }
    let layout = direct_section
        .map(DirectAtlasLayout::from_direct_section)
        .or(fallback_layout);
    let Some(layout) = layout else {
        return DirectAtlasUsage::default();
    };
    DirectAtlasUsage {
        needs_composed_atlas: true,
        needs_intermediate_atlas: has_animated_direct,
        atlas_dimensions: layout.atlas_dimensions,
        layer_count: layout.atlas_layer_count,
        layout: Some(layout),
    }
}

pub(super) fn atlas_fits(per_layer_dim: [u32; 2], layer_count: u32, limits: &wgpu::Limits) -> bool {
    per_layer_dim[0] > 0
        && per_layer_dim[1] > 0
        && layer_count > 0
        && per_layer_dim[0] <= limits.max_texture_dimension_2d
        && per_layer_dim[1] <= limits.max_texture_dimension_2d
        && layer_count <= limits.max_texture_array_layers
}

pub(super) fn volume_3d_fits(dimensions: [u32; 3], limits: &wgpu::Limits) -> bool {
    dimensions
        .iter()
        .all(|&dimension| dimension > 0 && dimension <= limits.max_texture_dimension_3d)
}

pub(super) fn storage_byte_len(
    element_count: usize,
    element_size: usize,
    empty_minimum: u64,
) -> Option<u64> {
    let bytes = u64::try_from(element_count)
        .ok()?
        .checked_mul(element_size as u64)?;
    Some(if bytes == 0 { empty_minimum } else { bytes })
}

pub(super) struct ComposeStoragePayloads {
    pub(super) delta_subblocks: StorageBufferPayload,
    pub(super) compaction_metadata: StorageBufferPayload,
    pub(super) affinity_offsets: StorageBufferPayload,
    pub(super) affinity_lights: StorageBufferPayload,
    pub(super) descriptor_indices: Option<StorageBufferPayload>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn compose_storage_payloads(
    delta_kind: ShAllocationKind,
    compaction_kind: ShAllocationKind,
    offsets_kind: ShAllocationKind,
    lights_kind: ShAllocationKind,
    descriptor_kind: Option<ShAllocationKind>,
    delta_subblocks: &[u16],
    compaction_metadata: &[u32],
    affinity_offsets: &[u32],
    affinity_lights: &[u32],
    descriptor_indices: Option<&[u32]>,
) -> ComposeStoragePayloads {
    let descriptor_indices = descriptor_kind
        .zip(descriptor_indices)
        .map(|(kind, indices)| StorageBufferPayload::new(kind, u32_slice_to_bytes(indices), 4));
    ComposeStoragePayloads {
        delta_subblocks: StorageBufferPayload::new(
            delta_kind,
            u16_slice_to_bytes(delta_subblocks),
            4,
        ),
        compaction_metadata: StorageBufferPayload::new(
            compaction_kind,
            u32_slice_to_bytes(compaction_metadata),
            4,
        ),
        affinity_offsets: StorageBufferPayload::new(
            offsets_kind,
            u32_slice_to_bytes(affinity_offsets),
            8,
        ),
        affinity_lights: StorageBufferPayload::new(
            lights_kind,
            u32_slice_to_bytes(affinity_lights),
            4,
        ),
        descriptor_indices,
    }
}

pub(super) fn billboard_storage_payloads(
    delta_rgba: &[u16],
    affinity_offsets: &[u32],
    affinity_lights: &[u32],
    descriptor_indices: &[u32],
) -> (
    StorageBufferPayload,
    StorageBufferPayload,
    StorageBufferPayload,
    StorageBufferPayload,
) {
    (
        StorageBufferPayload::new(
            ShAllocationKind::BillboardComposeDeltas,
            u16_slice_to_bytes(delta_rgba),
            4,
        ),
        StorageBufferPayload::new(
            ShAllocationKind::BillboardComposeOffsets,
            u32_slice_to_bytes(affinity_offsets),
            8,
        ),
        StorageBufferPayload::new(
            ShAllocationKind::BillboardComposeLights,
            u32_slice_to_bytes(affinity_lights),
            4,
        ),
        StorageBufferPayload::new(
            ShAllocationKind::BillboardComposeDescriptorIndices,
            u32_slice_to_bytes(descriptor_indices),
            4,
        ),
    )
}

pub(super) fn probe_indirection_storage_payload(
    kind: ShAllocationKind,
    words: &[u32],
) -> StorageBufferPayload {
    StorageBufferPayload::new(kind, u32_slice_to_bytes(words), 4)
}

pub(super) fn compose_origin_bytes(grid_origin: [f32; 3], cell_size: [f32; 3]) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[0..4].copy_from_slice(&grid_origin[0].to_ne_bytes());
    bytes[4..8].copy_from_slice(&grid_origin[1].to_ne_bytes());
    bytes[8..12].copy_from_slice(&grid_origin[2].to_ne_bytes());
    bytes[16..20].copy_from_slice(&cell_size[0].to_ne_bytes());
    bytes[20..24].copy_from_slice(&cell_size[1].to_ne_bytes());
    bytes[24..28].copy_from_slice(&cell_size[2].to_ne_bytes());
    bytes
}

pub(super) fn billboard_scatter_grid_bytes(
    grid_dimensions: [u32; 3],
    affinity_dimensions: [u32; 3],
) -> [u8; 32] {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indirect_base_allocation_uses_physical_bc6h_blocks() {
        let mut section = OctahedralShVolumeSection::placeholder();
        section.irradiance_format = IRRADIANCE_FORMAT_BC6H;
        section.atlas_dimensions = [9, 5];
        section.layer_count = 3;

        let allocation = indirect_base_atlas_allocation(Some(&section));
        assert_eq!(allocation.format, wgpu::TextureFormat::Bc6hRgbUfloat);
        assert_eq!(allocation.extent.width, 12);
        assert_eq!(allocation.extent.height, 8);
        assert_eq!(allocation.extent.depth_or_array_layers, 3);
    }

    #[test]
    fn empty_indirect_atlas_distinguishes_accepted_formats_from_missing_fallback() {
        let mut bc6h = OctahedralShVolumeSection::placeholder();
        bc6h.irradiance_format = IRRADIANCE_FORMAT_BC6H;
        bc6h.atlas_dimensions = [0, 0];
        bc6h.layer_count = 0;
        let bc6h_allocation = indirect_base_atlas_allocation(Some(&bc6h));
        assert_eq!(bc6h_allocation.format, wgpu::TextureFormat::Bc6hRgbUfloat);
        assert_eq!(bc6h_allocation.extent.width, 4);
        assert_eq!(bc6h_allocation.extent.height, 4);

        let mut rgba = OctahedralShVolumeSection::placeholder();
        rgba.irradiance_format = IRRADIANCE_FORMAT_RGBA16F;
        let rgba_allocation = indirect_base_atlas_allocation(Some(&rgba));
        assert_eq!(rgba_allocation.format, wgpu::TextureFormat::Rgba16Float);
        assert_eq!(rgba_allocation.extent.width, 1);
        assert_eq!(rgba_allocation.extent.height, 1);

        assert_eq!(
            indirect_base_atlas_allocation(None),
            indirect_base_atlas_dummy_allocation()
        );
    }

    #[test]
    fn storage_payload_padding_keeps_empty_csr_offsets_readable() {
        let payloads = compose_storage_payloads(
            ShAllocationKind::IndirectComposeDeltaSubblocks,
            ShAllocationKind::IndirectComposeCompactionMetadata,
            ShAllocationKind::IndirectComposeAffinityOffsets,
            ShAllocationKind::IndirectComposeAffinityLights,
            Some(ShAllocationKind::IndirectComposeDescriptorIndices),
            &[],
            &[],
            &[],
            &[],
            Some(&[]),
        );
        assert_eq!(payloads.delta_subblocks.allocation.byte_len, 4);
        assert_eq!(payloads.compaction_metadata.allocation.byte_len, 4);
        assert_eq!(payloads.affinity_offsets.allocation.byte_len, 8);
        assert_eq!(payloads.affinity_lights.allocation.byte_len, 4);
        assert_eq!(
            payloads
                .descriptor_indices
                .expect("indirect compose owns descriptor indices")
                .allocation
                .byte_len,
            4
        );
    }

    #[test]
    fn indirect_total_atlas_is_one_storage_and_sampled_texture_request() {
        let allocation = indirect_total_atlas_allocation([7, 9], 2, false);
        assert_eq!(allocation.kind, ShAllocationKind::IndirectTotalAtlas);
        assert_eq!(allocation.extent.depth_or_array_layers, 2);
        assert!(
            allocation
                .usage
                .contains(wgpu::TextureUsages::STORAGE_BINDING)
        );
        assert!(
            allocation
                .usage
                .contains(wgpu::TextureUsages::TEXTURE_BINDING)
        );
    }
}
