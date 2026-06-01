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
/// Non-filtering (Nearest) sampler. Used for the octahedral direction texture:
/// linear interpolation of octahedral-encoded unit vectors does not commute
/// with slerp, so the direction channel must stay nearest.
pub const BIND_SAMPLER: u32 = 2;
/// Animated-light contribution atlas (Rgba16Float). Composed each frame by
/// `render::animated_lightmap`; forward pass samples alongside the static
/// atlas. See animated-light-weight-maps/ §Task 5.
pub const BIND_ANIMATED_ATLAS: u32 = 3;
/// Filtering (Linear) sampler. Used for the irradiance and animated atlases so
/// baked penumbra ramps read as continuous gradients under magnification
/// instead of stair-stepping at atlas-texel boundaries. See
/// baked-soft-lightmap-shadows/ §Task 5. Bound in every variant; on the manual
/// 4-tap fallback path it goes unused (the shader sees `use_hw_filter == false`)
/// but stays bound so the group-4 BGL is identical across both pipeline variants.
pub const BIND_FILTERING_SAMPLER: u32 = 4;

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
    /// Static dominant-direction atlas texture (Rgba8Unorm, octahedral in rg).
    /// Its sole consumer — the SDF pass's static dominant-direction trace — was
    /// removed in `sdf-per-light-shadows` Task 2 (per-light static shadows now
    /// key on light position). The baked atlas is still uploaded; retiring it
    /// from the bake/upload path is the follow-on once animated lights also
    /// migrate off the baked trace (see the plan's architecture map "Defers").
    #[allow(dead_code)]
    direction_texture: wgpu::Texture,
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
        animated_atlas_view: &wgpu::TextureView,
    ) -> Self {
        // Nearest sampler for the octahedral direction texture (binding 1):
        // linear interpolation of octahedral-encoded unit vectors does not
        // commute with slerp, so direction must stay nearest.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Lightmap Sampler (Nearest)"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // Linear sampler for the irradiance + animated atlases (both
        // Rgba16Float, which is filterable in core WebGPU). Turns baked
        // multi-texel penumbra ramps into continuous gradients under
        // magnification. On the manual 4-tap fallback path the shader ignores
        // this binding, but it stays bound so the group-4 BGL is identical
        // across both pipeline variants. See baked-soft-lightmap-shadows/ §Task 5.
        let filtering_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Lightmap Sampler (Linear)"),
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
                wgpu::BindGroupEntry {
                    binding: BIND_ANIMATED_ATLAS,
                    resource: wgpu::BindingResource::TextureView(animated_atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_FILTERING_SAMPLER,
                    resource: wgpu::BindingResource::Sampler(&filtering_sampler),
                },
            ],
        });

        Self {
            bind_group,
            present,
            direction_texture: direction_tex,
        }
    }
}

fn bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 5] {
    // Two samplers (binding 2 nearest, binding 4 linear), split by what each
    // texture needs:
    //   - Irradiance (0) and animated atlas (3) are `Rgba16Float`, which is
    //     filterable in core WebGPU (only 32-bit float formats need the
    //     `float32-filterable` feature). Marked `filterable: true` and sampled
    //     through the linear sampler so baked penumbra ramps read as continuous
    //     gradients instead of stair-stepping at texel boundaries.
    //   - Direction (1) stays `filterable: false` on the nearest sampler:
    //     linear interpolation of octahedral-encoded unit vectors does not
    //     commute with slerp.
    // The BGL is identical across the HW-filter and 4-tap-fallback pipeline
    // variants — the fallback simply leaves the linear sampler unused.
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
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: BIND_SAMPLER,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
            count: None,
        },
        // Animated-light contribution atlas (Rgba16Float) — filterable in core
        // WebGPU, sampled through the linear sampler at binding 4 (HW path) or
        // a manual 4-tap lerp (fallback).
        wgpu::BindGroupLayoutEntry {
            binding: BIND_ANIMATED_ATLAS,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: BIND_FILTERING_SAMPLER,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ]
}

/// Whether `Rgba16Float` (the irradiance + animated atlas format) advertises
/// hardware bilinear filtering on this adapter. Decides the forward pipeline's
/// `use_hw_filter` override once at init: `true` → sample through the linear
/// sampler; `false` → manual 4-tap bilinear lerp in `forward.wgsl`. Both produce
/// the same continuous ramp; the fallback only costs extra per-fragment work.
pub fn atlas_format_filterable(adapter: &wgpu::Adapter) -> bool {
    adapter
        .get_texture_format_features(wgpu::TextureFormat::Rgba16Float)
        .flags
        .contains(wgpu::TextureFormatFeatureFlags::FILTERABLE)
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

fn upload_placeholder_irradiance(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::Texture {
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

fn upload_placeholder_direction(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::Texture {
    // Neutral direction: +Y encoded octahedral (0, 1) maps to (0.5, 1.0) →
    // 8-bit quantization (128, 255). Alpha 0xFF.
    let bytes = [128u8, 255, 128, 255];
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
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &bytes,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // The group-4 BGL is a fixed contract with `forward.wgsl`'s `@binding`
    // decorators. This pins the sampler split decided in Task 5: which textures
    // are filterable, and the two sampler bindings (nearest + linear).
    #[test]
    fn bgl_entries_pin_sampler_split() {
        let entries = bind_group_layout_entries();
        assert_eq!(entries.len(), 5, "group-4 BGL grew to 5 entries");

        let tex_sample = |b: u32| {
            entries
                .iter()
                .find(|e| e.binding == b)
                .and_then(|e| match e.ty {
                    wgpu::BindingType::Texture { sample_type, .. } => Some(sample_type),
                    _ => None,
                })
        };
        let sampler_ty = |b: u32| {
            entries
                .iter()
                .find(|e| e.binding == b)
                .and_then(|e| match e.ty {
                    wgpu::BindingType::Sampler(t) => Some(t),
                    _ => None,
                })
        };

        // Irradiance + animated atlas filter linear (Rgba16Float is filterable).
        assert_eq!(
            tex_sample(BIND_IRRADIANCE),
            Some(wgpu::TextureSampleType::Float { filterable: true })
        );
        assert_eq!(
            tex_sample(BIND_ANIMATED_ATLAS),
            Some(wgpu::TextureSampleType::Float { filterable: true })
        );
        // Direction stays nearest (octahedral lerp ≠ slerp).
        assert_eq!(
            tex_sample(BIND_DIRECTION),
            Some(wgpu::TextureSampleType::Float { filterable: false })
        );
        // Two samplers: nearest at binding 2, linear at binding 4.
        assert_eq!(
            sampler_ty(BIND_SAMPLER),
            Some(wgpu::SamplerBindingType::NonFiltering)
        );
        assert_eq!(
            sampler_ty(BIND_FILTERING_SAMPLER),
            Some(wgpu::SamplerBindingType::Filtering)
        );
    }
}
