// Plain-Rust GPU residency rows shared by the SH and lightmap-family reports.
// See: context/lib/rendering_pipeline.md §7.8

/// A PRL section or renderer-derived input cited by a residency row.
///
/// Reports intentionally carry no `wgpu` handles or descriptors: capture and
/// other non-renderer consumers only need the allocation decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidencySource {
    Section(u16),
    Derived,
}

/// Whether the allocation backs accepted source data, a required valid GPU
/// binding for an absent source, or a replacement for a rejected source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidencyAllocationState {
    Data,
    Dummy,
    Fallback,
}

/// Requested physical shape of a level-owned allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResidencyAllocationShape {
    Texture {
        format: &'static str,
        dimension: &'static str,
        extent: [u32; 3],
    },
    Buffer {
        binding_bytes: u64,
    },
}

/// One physical texture or buffer created for the installed level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidencyAllocation {
    pub name: &'static str,
    pub sources: Vec<ResidencySource>,
    pub bytes: u64,
    pub state: ResidencyAllocationState,
    pub shape: ResidencyAllocationShape,
}

pub(crate) fn allocation_total_bytes(allocations: &[ResidencyAllocation]) -> u64 {
    allocations
        .iter()
        .try_fold(0_u64, |total, row| total.checked_add(row.bytes))
        .expect("residency static allocation total overflow")
}

pub(crate) fn source_ids<const N: usize>(ids: [Option<u16>; N]) -> Vec<u16> {
    ids.into_iter().flatten().collect()
}

pub(crate) fn sources(section_ids: &[u16], derived: bool) -> Vec<ResidencySource> {
    let mut sources = section_ids
        .iter()
        .copied()
        .map(ResidencySource::Section)
        .collect::<Vec<_>>();
    if derived || sources.is_empty() {
        sources.push(ResidencySource::Derived);
    }
    sources
}

/// Row for a texture that already exists, read back from the texture itself
/// so the report describes exactly what is bound rather than what a
/// constructor intended before a fallback.
pub(crate) fn texture_row(
    name: &'static str,
    texture: &wgpu::Texture,
    section_ids: &[u16],
    state: ResidencyAllocationState,
) -> ResidencyAllocation {
    let extent = texture.size();
    let format = texture.format();
    ResidencyAllocation {
        name,
        sources: sources(section_ids, false),
        bytes: texture_bytes(format, extent),
        state,
        shape: ResidencyAllocationShape::Texture {
            format: texture_format_name(format),
            dimension: texture_dimension_name(texture.dimension()),
            extent: [extent.width, extent.height, extent.depth_or_array_layers],
        },
    }
}

/// Bytes of a single-mip texture: whole compression blocks times block size.
pub(crate) fn texture_bytes(format: wgpu::TextureFormat, extent: wgpu::Extent3d) -> u64 {
    let (block_w, block_h) = format.block_dimensions();
    let block_bytes = format
        .block_copy_size(None)
        .unwrap_or_else(|| panic!("residency texture format {format:?} has no copy size"));
    u64::from(extent.width.div_ceil(block_w))
        * u64::from(extent.height.div_ceil(block_h))
        * u64::from(extent.depth_or_array_layers)
        * u64::from(block_bytes)
}

pub(crate) fn texture_format_name(format: wgpu::TextureFormat) -> &'static str {
    match format {
        wgpu::TextureFormat::Bc5RgUnorm => "Bc5RgUnorm",
        wgpu::TextureFormat::Bc6hRgbUfloat => "Bc6hRgbUfloat",
        wgpu::TextureFormat::Rg8Unorm => "Rg8Unorm",
        wgpu::TextureFormat::Rgba8Unorm => "Rgba8Unorm",
        wgpu::TextureFormat::Rgba16Float => "Rgba16Float",
        wgpu::TextureFormat::Rgba16Uint => "Rgba16Uint",
        format => panic!("unexpected residency texture format {format:?}"),
    }
}

pub(crate) fn texture_dimension_name(dimension: wgpu::TextureDimension) -> &'static str {
    match dimension {
        wgpu::TextureDimension::D2 => "D2",
        wgpu::TextureDimension::D3 => "D3",
        wgpu::TextureDimension::D1 => "D1",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_bytes_counts_whole_compression_blocks_and_plain_texels() {
        let extent = wgpu::Extent3d {
            width: 5,
            height: 7,
            depth_or_array_layers: 2,
        };
        // BC blocks are 4×4 at 16 B: ceil(5/4) × ceil(7/4) × 2 layers.
        assert_eq!(
            texture_bytes(wgpu::TextureFormat::Bc6hRgbUfloat, extent),
            2 * 2 * 2 * 16
        );
        assert_eq!(
            texture_bytes(wgpu::TextureFormat::Bc5RgUnorm, extent),
            2 * 2 * 2 * 16
        );
        assert_eq!(
            texture_bytes(wgpu::TextureFormat::Rgba16Float, extent),
            5 * 7 * 2 * 8
        );
        assert_eq!(
            texture_bytes(wgpu::TextureFormat::Rgba8Unorm, extent),
            5 * 7 * 2 * 4
        );
        assert_eq!(
            texture_bytes(wgpu::TextureFormat::Rg8Unorm, extent),
            5 * 7 * 2 * 2
        );
    }
}
