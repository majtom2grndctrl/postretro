// Lightmap GPU texture uploads: irradiance, direction and shadowmask atlases,
// their placeholders, and the adapter format checks.
// See: context/lib/rendering_pipeline.md §4

use postretro_level_format::lightmap::{
    DIRECTION_FORMAT_OCT_RG8, DIRECTION_FORMAT_OCT_RGBA8, IRRADIANCE_FORMAT_BC6H, LightmapHeader,
};
use postretro_level_format::shadowmask_atlas::{SHADOWMASK_GROUP_COUNT, ShadowmaskAtlasHeader};
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

pub(super) fn upload_irradiance_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sec: &LightmapHeader,
    irradiance: &[u8],
) -> wgpu::Texture {
    // Branch the texture format on the section's stored tag. Both formats bind
    // through the same group-4 BGL slot (`Float { filterable: true }`) and
    // sample through the existing linear sampler — `Bc6hRgbUfloat` is hardware-
    // decoded before filtering, so the shader's sample call is identical and
    // requires no second pipeline variant. RGBA16F retains its alpha (legacy
    // padding); BC6H is RGB-only and the shader's `.rgb` swizzle never reads
    // alpha. `create_texture_with_data` accepts the block-compressed payload
    // verbatim — the dimensions are the texel-space size and the data slice is
    // `ceil(w/4)·ceil(h/4)·16` bytes.
    let format = match sec.irradiance_format {
        IRRADIANCE_FORMAT_BC6H => wgpu::TextureFormat::Bc6hRgbUfloat,
        // `IRRADIANCE_FORMAT_RGBA16F` (or any value `from_bytes` already
        // gated to one of the two known tags).
        _ => wgpu::TextureFormat::Rgba16Float,
    };
    // `texture_2d_array`: the on-disk `irradiance` blob is layer-major (layer 0's
    // texels, then layer 1's, …), exactly the `LayerMajor` order
    // `create_texture_with_data` expects, so a single upload covers all layers.
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("Lightmap Irradiance"),
            size: wgpu::Extent3d {
                width: sec.irr_width,
                height: sec.irr_height,
                depth_or_array_layers: sec.layer_count,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        irradiance,
    )
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

pub(super) fn upload_direction_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sec: &LightmapHeader,
    direction: &[u8],
) -> wgpu::Texture {
    // `texture_2d_array`, sharing `layer_count` with the irradiance atlas. The
    // `direction` blob is layer-major, so one `LayerMajor` upload covers all layers.
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("Lightmap Direction"),
            size: wgpu::Extent3d {
                width: sec.dir_width,
                height: sec.dir_height,
                depth_or_array_layers: sec.layer_count,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: direction_texture_format(sec.direction_format),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        direction,
    )
}

/// Select the texture format from the strictly parsed section tag. Current
/// compilers write Rg8; Rgba8 remains loadable for existing PRLs whose static
/// direction bytes carried unused padding channels.
pub(super) fn direction_texture_format(direction_format: u32) -> wgpu::TextureFormat {
    match direction_format {
        DIRECTION_FORMAT_OCT_RG8 => wgpu::TextureFormat::Rg8Unorm,
        DIRECTION_FORMAT_OCT_RGBA8 => wgpu::TextureFormat::Rgba8Unorm,
        unknown => panic!("unsupported lightmap direction format tag {unknown}"),
    }
}

/// The texture a usable shadowmask section uploads as: BC5 `.rg`, both mask
/// groups side by side, one layer per lightmap layer. Callers pass a section
/// that `filter_usable_shadowmask_section` kept.
pub(crate) fn shadowmask_texture_descriptor(
    sec: &ShadowmaskAtlasHeader,
) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("Shadowmask Atlas"),
        size: wgpu::Extent3d {
            width: sec
                .texture_width()
                .expect("usable shadowmask width fits the device texture limit"),
            height: sec.height,
            depth_or_array_layers: sec.layer_count,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bc5RgUnorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    }
}

pub(crate) fn upload_shadowmask_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sec: &ShadowmaskAtlasHeader,
    data: &[u8],
) -> wgpu::Texture {
    // The payload is layer-major BC5 blocks, exactly the `LayerMajor` order
    // `create_texture_with_data` expects for a block-compressed array.
    device.create_texture_with_data(
        queue,
        &shadowmask_texture_descriptor(sec),
        wgpu::util::TextureDataOrder::LayerMajor,
        data,
    )
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
