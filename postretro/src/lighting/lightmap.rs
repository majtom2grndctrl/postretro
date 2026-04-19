// Directional lightmap GPU resources: irradiance + direction atlas upload,
// sampler, and bind group (group 4).
//
// See: context/plans/ready/lighting-lightmaps/index.md
//      context/lib/rendering_pipeline.md §4

use postretro_level_format::lightmap::LightmapSection;
use wgpu::util::DeviceExt;

/// Group 4 bindings. The layout is fixed — the fragment shader's
/// `@binding` decorators must match these values.
pub const BIND_IRRADIANCE: u32 = 0;
pub const BIND_DIRECTION: u32 = 1;
pub const BIND_SAMPLER: u32 = 2;

/// GPU-side lightmap atlas: irradiance texture, direction texture, sampler,
/// and the bind group that exposes them to the forward shader.
///
/// Always allocated. When the level has no `Lightmap` PRL section, a 1×1
/// white/neutral placeholder is uploaded so the shader path is identical in
/// every case. That matches the runtime fallback the SH volume uses and keeps
/// the bind group layout independent of map content.
///
/// The bind-group-layout is returned separately from `new()` because the
/// pipeline layout needs it before the bind group is populated — storing it
/// alongside the bind group would have two owners of the same logical handle.
pub struct LightmapResources {
    pub bind_group: wgpu::BindGroup,
    /// Whether a real lightmap atlas was uploaded (false = 1×1 placeholder).
    /// Read by future debug UIs; kept public so it doesn't drift with dead-code
    /// elimination in release builds.
    #[allow(dead_code)]
    pub present: bool,
}

/// Build the lightmap bind group layout. Callable before resources exist so
/// the pipeline layout can be assembled up front.
pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Lightmap Bind Group Layout"),
        entries: &bind_group_layout_entries(),
    })
}

impl LightmapResources {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        section: Option<&LightmapSection>,
        bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Lightmap Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let usable = section.filter(|s| s.width > 0 && s.height > 0);
        let present = usable.is_some();

        let (irradiance_tex, direction_tex) = match usable {
            Some(sec) => (
                upload_irradiance_texture(device, queue, sec),
                upload_direction_texture(device, queue, sec),
            ),
            None => (
                upload_placeholder_irradiance(device, queue),
                upload_placeholder_direction(device, queue),
            ),
        };

        let irr_view = irradiance_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let dir_view = direction_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lightmap Bind Group"),
            layout: bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: BIND_IRRADIANCE,
                    resource: wgpu::BindingResource::TextureView(&irr_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_DIRECTION,
                    resource: wgpu::BindingResource::TextureView(&dir_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_SAMPLER,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        Self {
            bind_group,
            present,
        }
    }
}

fn bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: BIND_IRRADIANCE,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: BIND_DIRECTION,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: BIND_SAMPLER,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ]
}

fn upload_irradiance_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sec: &LightmapSection,
) -> wgpu::Texture {
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("Lightmap Irradiance"),
            size: wgpu::Extent3d {
                width: sec.width,
                height: sec.height,
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
        &sec.irradiance,
    )
}

fn upload_direction_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sec: &LightmapSection,
) -> wgpu::Texture {
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("Lightmap Direction"),
            size: wgpu::Extent3d {
                width: sec.width,
                height: sec.height,
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
        &sec.direction,
    )
}

fn upload_placeholder_irradiance(
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
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
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

fn upload_placeholder_direction(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> wgpu::Texture {
    // Neutral direction: +Y encoded octahedral (0, 1) maps to (0.5, 1.0) →
    // 8-bit quantization (128, 255). Alpha 0xFF.
    let bytes = [128u8, 255, 128, 255];
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("Lightmap Direction Placeholder"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
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
