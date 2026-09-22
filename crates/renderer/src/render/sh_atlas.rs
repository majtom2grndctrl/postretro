// SH atlas texture upload and creation helpers.
// See: context/lib/rendering_pipeline.md §4

use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_render_cpu::sh_compose::u16_slice_to_bytes;
use wgpu::util::DeviceExt;

use super::sh_allocation::{TextureAllocation, indirect_base_atlas_empty_payload};

/// Upload v11's node-aware base-volume stored-tile atlas without re-expanding it.
/// BC6H blobs remain compressed through upload and hardware-decode only in the
/// compose pass; the uncompressed debug tag keeps its compact `Rgba16Float` texels.
pub(super) fn upload_compact_base_atlas_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    section: &OctahedralShVolumeSection,
    allocation: TextureAllocation,
) -> wgpu::Texture {
    let empty_compact_atlas = indirect_base_atlas_empty_payload(section);
    let zero_bc6h = [0u8; 16];
    let zero_rgba16f = [0u8; 8];
    let contents = match section.irradiance_format {
        IRRADIANCE_FORMAT_BC6H if empty_compact_atlas => zero_bc6h.as_slice(),
        IRRADIANCE_FORMAT_RGBA16F if empty_compact_atlas => zero_rgba16f.as_slice(),
        IRRADIANCE_FORMAT_BC6H | IRRADIANCE_FORMAT_RGBA16F => section.compact_atlas.as_slice(),
        unknown => panic!("unsupported compact SH irradiance format tag {unknown}"),
    };

    device.create_texture_with_data(
        queue,
        &allocation.descriptor(Some("SH Base Octahedral Atlas")),
        wgpu::util::TextureDataOrder::LayerMajor,
        contents,
    )
}

/// Dummy for the no-usable-probes path. It is `Rgba16Float` because every
/// compose indirection word is a sentinel, so the texture is never sampled.
pub(super) fn upload_compact_base_atlas_dummy(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    allocation: TextureAllocation,
) -> wgpu::Texture {
    let zero_texel = [0u8; 8];
    device.create_texture_with_data(
        queue,
        &allocation.descriptor(Some("SH Base Octahedral Atlas Dummy")),
        wgpu::util::TextureDataOrder::LayerMajor,
        &zero_texel,
    )
}

#[cfg(feature = "dev-tools")]
pub(super) fn base_atlas_format_label(format: wgpu::TextureFormat) -> &'static str {
    match format {
        wgpu::TextureFormat::Bc6hRgbUfloat => "BC6H",
        wgpu::TextureFormat::Rgba16Float => "Rgba16Float",
        _ => unreachable!("base SH atlas allocation only uses BC6H or Rgba16Float"),
    }
}

pub(super) fn upload_depth_moment_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data_u16: &[u16],
    allocation: TextureAllocation,
) -> wgpu::Texture {
    let size = allocation.extent;
    let texture = device.create_texture(&allocation.descriptor(Some("SH Depth Moments")));
    let byte_slice = u16_slice_to_bytes(data_u16);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &byte_slice,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8 * size.width),
            rows_per_image: Some(size.height),
        },
        size,
    );
    texture
}

/// Create the stored-tile total octahedral atlas texture. No data is uploaded
/// — wgpu zero-initializes; the compose pass overwrites every stored texel.
pub(super) fn create_total_atlas_texture(
    device: &wgpu::Device,
    allocation: TextureAllocation,
    label: &str,
) -> wgpu::Texture {
    device.create_texture(&allocation.descriptor(Some(label)))
}
