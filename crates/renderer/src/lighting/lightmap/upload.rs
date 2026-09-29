// Lightmap placeholder textures (placeholder mode's neutral irradiance,
// direction and all-visible shadowmask) and the adapter format checks.
// See: context/lib/rendering_pipeline.md §4

use postretro_level_format::shadowmask_atlas::SHADOWMASK_GROUP_COUNT;
use wgpu::util::DeviceExt;

/// Whether `Rgba16Float` (the irradiance + animated atlas format) advertises
/// hardware bilinear filtering on this adapter. Checked once at init: the
/// forward pass samples the irradiance + animated atlases through the linear
/// sampler, so a non-filterable adapter is rejected (see `Renderer::new`).
/// Linear 16-bit-float filtering is core WebGPU and mandated on all targeted
/// backends, so this holds everywhere the engine is supported.
pub fn atlas_format_filterable(adapter: &wgpu::Adapter) -> bool {
    adapter
        .get_texture_format_features(wgpu::TextureFormat::Rgba16Float)
        .flags
        .contains(wgpu::TextureFormatFeatureFlags::FILTERABLE)
}

/// Whether `Bc6hRgbUfloat` (the default irradiance storage on disk) advertises
/// the texture-binding and linear-filtering features the runtime relies on.
/// Mirrors `atlas_format_filterable` for the BC6H sibling check: BC6H is the
/// default storage and a `TEXTURE_COMPRESSION_BC`-granted adapter that fails
/// to advertise filterable BC6H here would fail later at bind-group creation
/// with an opaque error. `TEXTURE_COMPRESSION_BC` is already a required
/// feature (see `render::renderer_init_resources.rs`'s adapter pre-check); this
/// check confirms the format that feature unlocks supports the usages we need.
pub fn bc6h_irradiance_filterable(adapter: &wgpu::Adapter) -> bool {
    adapter
        .get_texture_format_features(wgpu::TextureFormat::Bc6hRgbUfloat)
        .flags
        .contains(wgpu::TextureFormatFeatureFlags::FILTERABLE)
}

pub(super) fn upload_placeholder_irradiance(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> wgpu::Texture {
    // 1×1 white RGBA16Float texel (1.0, 1.0, 1.0, 1.0). f16(1.0) = 0x3c00.
    let white = 0x3c00u16;
    let mut bytes = Vec::with_capacity(8);
    for _ in 0..4 {
        bytes.extend_from_slice(&white.to_le_bytes());
    }
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("Lightmap Irradiance Placeholder"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &bytes,
    )
}

pub(super) fn upload_placeholder_direction(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> wgpu::Texture {
    // Neutral direction: +Y encoded octahedral (0, 1) maps to (0.5, 1.0) →
    // 8-bit quantization (128, 255).
    let bytes = [128u8, 255];
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("Lightmap Direction Placeholder"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &bytes,
    )
}

pub(crate) fn upload_placeholder_shadowmask(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> wgpu::Texture {
    // Two white texels: the shader splits the width into two mask groups, so
    // each group reads one real, fully visible texel.
    let bytes = [255u8; 8];
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("Shadowmask Atlas Placeholder"),
            size: wgpu::Extent3d {
                width: SHADOWMASK_GROUP_COUNT,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &bytes,
    )
}
