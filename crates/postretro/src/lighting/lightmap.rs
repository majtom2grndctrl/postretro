// Directional lightmap GPU resources: irradiance + direction atlas upload,
// sampler, and bind group (group 4).
// See: context/lib/rendering_pipeline.md §4

use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, LightmapSection};
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
/// atlas. See: context/lib/rendering_pipeline.md §4
pub const BIND_ANIMATED_ATLAS: u32 = 3;
/// Filtering (Linear) sampler. Used for the irradiance and animated atlases so
/// baked penumbra ramps read as continuous gradients under magnification
/// instead of stair-stepping at atlas-texel boundaries. `Rgba16Float`
/// linear-filterability is a hard runtime requirement checked at init
/// (see `atlas_format_filterable`; see also `rendering_pipeline.md §4`).
pub const BIND_FILTERING_SAMPLER: u32 = 4;
/// Animated dominant-direction atlas (Rgba16Float, raw normalized vec3). Composed
/// each frame by `render::animated_lightmap` alongside the animated irradiance
/// atlas; the forward pass samples it to apply the same bumped-Lambert normal-map
/// correction to the animated term that the static term already receives. Sampled
/// through the nearest sampler at binding 2 — like the static direction atlas,
/// directions must not be linearly interpolated. This group-4 binding (5) and the
/// compose shader's storage binding (8) are independent numbering spaces for the
/// same atlas.
pub const BIND_ANIMATED_DIRECTION: u32 = 5;

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
        animated_direction_view: &wgpu::TextureView,
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
        // Rgba16Float). Turns baked penumbra ramps into continuous gradients
        // under magnification. Always used — Rgba16Float linear-filterability
        // is a hard runtime requirement; non-filterable adapters are rejected
        // at init (see `atlas_format_filterable`). See rendering_pipeline.md §4.
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

        // Defensive runtime guard: the init adapter pre-check guarantees the
        // device grants at least 8192² (see `render::mod.rs`), and the bake's
        // `MAX_ATLAS_DIMENSION` matches that ceiling. A baked atlas larger than
        // the granted limit can only come from future content or a corrupt
        // section. Drop to the neutral placeholder with a logged error rather
        // than panicking on texture creation. Mirrors `render::sh_volume`'s
        // atlas-fits-device filter.
        let usable = filter_usable_section(section, device.limits().max_texture_dimension_2d);
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
                wgpu::BindGroupEntry {
                    binding: BIND_ANIMATED_DIRECTION,
                    resource: wgpu::BindingResource::TextureView(animated_direction_view),
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

fn bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 6] {
    // Two samplers (binding 2 nearest, binding 4 linear), split by what each
    // texture needs:
    //   - Irradiance (0) and animated atlas (3) are `Rgba16Float`, which is
    //     filterable in core WebGPU (only 32-bit float formats need the
    //     `float32-filterable` feature). Marked `filterable: true` and always
    //     sampled through the linear sampler so baked penumbra ramps read as
    //     continuous gradients instead of stair-stepping at texel boundaries.
    //   - Direction (1) and animated direction (5) stay `filterable: false` on
    //     the nearest sampler: linear interpolation of direction vectors does
    //     not commute with slerp (the static atlas is octahedral-encoded; the
    //     animated atlas stores a raw normalized vec3 — both must read nearest).
    // There is one pipeline variant; the BGL is fixed. No fallback path.
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
        // WebGPU, always sampled through the linear sampler at binding 4.
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
        // Animated dominant-direction atlas (Rgba16Float, raw normalized vec3) —
        // `filterable: false` and sampled through the nearest sampler at binding
        // 2, mirroring the static direction atlas (1). Linear interpolation of
        // direction vectors does not commute with slerp.
        wgpu::BindGroupLayoutEntry {
            binding: BIND_ANIMATED_DIRECTION,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
    ]
}

/// Resolve the dimensions the static irradiance/direction atlases are actually
/// created at, applying the same usability filter `new()` uses. Returns `None`
/// when the section is absent, zero-area, or oversize — i.e. exactly when the
/// static path falls back to the 1×1 placeholder.
///
/// The animated-lightmap atlases must be created at these same dimensions: the
/// compose pass writes them at absolute static-atlas coordinates (baked
/// `ChunkAtlasRect`s) and the forward pass samples all three atlases with one
/// normalized `lightmap_uv`, so a size mismatch drops out-of-range writes and
/// misaligns the in-range ones. Routing both through this one function keeps the
/// animated atlas size locked to the static atlas size.
pub(crate) fn usable_atlas_dimensions(
    section: Option<&LightmapSection>,
    max_texture_dimension_2d: u32,
) -> Option<(u32, u32)> {
    filter_usable_section(section, max_texture_dimension_2d).map(|s| (s.width, s.height))
}

/// Filter out an absent (`None`), zero-dimension, or oversize `LightmapSection`,
/// returning `None` so the caller falls through to the neutral placeholder. Pure
/// dimension-vs-limit comparison — unit-testable without a real wgpu device.
fn filter_usable_section(
    section: Option<&LightmapSection>,
    max_texture_dimension_2d: u32,
) -> Option<&LightmapSection> {
    section.filter(|s| s.width > 0 && s.height > 0).filter(|s| {
        let fits = s.width <= max_texture_dimension_2d && s.height <= max_texture_dimension_2d;
        if !fits {
            log::error!(
                "[Renderer] Lightmap atlas {}x{} exceeds device maxTextureDimension2D {}; \
                     degrading to neutral placeholder for this level",
                s.width,
                s.height,
                max_texture_dimension_2d,
            );
        }
        fits
    })
}

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

fn upload_irradiance_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sec: &LightmapSection,
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
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &sec.irradiance,
    )
}

/// Whether `Bc6hRgbUfloat` (the default irradiance storage on disk) advertises
/// the texture-binding and linear-filtering features the runtime relies on.
/// Mirrors `atlas_format_filterable` for the BC6H sibling check: BC6H is the
/// default storage and a `TEXTURE_COMPRESSION_BC`-granted adapter that fails
/// to advertise filterable BC6H here would fail later at bind-group creation
/// with an opaque error. `TEXTURE_COMPRESSION_BC` is already a required
/// feature (see `render::mod.rs`'s adapter pre-check); this check confirms
/// the format that feature unlocks supports the usages we need.
pub fn bc6h_irradiance_filterable(adapter: &wgpu::Adapter) -> bool {
    adapter
        .get_texture_format_features(wgpu::TextureFormat::Bc6hRgbUfloat)
        .flags
        .contains(wgpu::TextureFormatFeatureFlags::FILTERABLE)
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
    use postretro_level_format::lightmap::LightmapMode;

    fn fake_section(width: u32, height: u32) -> LightmapSection {
        LightmapSection {
            width,
            height,
            texel_density: 0.04,
            irradiance: Vec::new(),
            irradiance_format: postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F,
            direction: Vec::new(),
            mode: LightmapMode::Shadowed,
        }
    }

    /// Atlas-fits-device guard: a section that exceeds the granted
    /// `max_texture_dimension_2d` is dropped so the caller falls through to the
    /// neutral placeholder, rather than panicking on texture creation. Pure
    /// dimension comparison — no real oversize allocation needed.
    #[test]
    fn oversize_section_filtered_out() {
        let oversize = fake_section(16_384, 8192);
        assert!(
            filter_usable_section(Some(&oversize), 8192).is_none(),
            "atlas wider than the granted limit must drop to placeholder",
        );

        let tall = fake_section(8192, 16_384);
        assert!(
            filter_usable_section(Some(&tall), 8192).is_none(),
            "atlas taller than the granted limit must drop to placeholder",
        );
    }

    /// Task 4a — the over-limit degrade AC requires not just a drop-to-
    /// placeholder, but a logged `[Renderer]`-prefixed error so triage can
    /// trace the silent flat-ambient fall-through. Capture the log records
    /// emitted during the filter call and assert the prefix + the format the
    /// AC pins (no real oversize allocation; the test runs against the pure
    /// dimension-comparison path).
    #[test]
    fn oversize_section_logs_renderer_prefixed_error() {
        let oversize = fake_section(16_384, 4096);
        // Capture log records on this thread, scoped to the filter call.
        let records = crate::scripting::reactions::log_capture::capture(|| {
            let _ = filter_usable_section(Some(&oversize), 8192);
        });
        assert!(
            records
                .iter()
                .any(|(level, msg)| *level == log::Level::Error
                    && msg.starts_with("[Renderer]")
                    && msg.contains("Lightmap atlas")
                    && msg.contains("16384")
                    && msg.contains("8192")),
            "expected a `[Renderer]`-prefixed error naming the oversize atlas and the granted \
             limit; got records: {records:?}",
        );
    }

    #[test]
    fn at_or_under_limit_section_kept() {
        let at_limit = fake_section(8192, 8192);
        assert!(
            filter_usable_section(Some(&at_limit), 8192).is_some(),
            "atlas exactly at the granted limit must be retained",
        );

        let under = fake_section(4096, 2048);
        assert!(
            filter_usable_section(Some(&under), 8192).is_some(),
            "atlas under the granted limit must be retained",
        );
    }

    #[test]
    fn zero_dimension_section_filtered_out() {
        let empty = fake_section(0, 0);
        assert!(
            filter_usable_section(Some(&empty), 8192).is_none(),
            "zero-dimension section must drop to placeholder regardless of limit",
        );
    }

    #[test]
    fn missing_section_filtered_out() {
        assert!(
            filter_usable_section(None, 8192).is_none(),
            "absent section drops to placeholder",
        );
    }

    // The group-4 BGL is a fixed contract with `forward.wgsl`'s `@binding`
    // decorators. This pins the sampler split decided in Task 5: which textures
    // are filterable, and the two sampler bindings (nearest + linear).
    #[test]
    fn bgl_entries_pin_sampler_split() {
        let entries = bind_group_layout_entries();
        assert_eq!(entries.len(), 6, "group-4 BGL grew to 6 entries");

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
        // Both direction atlases stay nearest (direction lerp ≠ slerp): the
        // static atlas (1) is octahedral-encoded, the animated atlas (5) stores
        // a raw normalized vec3.
        assert_eq!(
            tex_sample(BIND_DIRECTION),
            Some(wgpu::TextureSampleType::Float { filterable: false })
        );
        assert_eq!(
            tex_sample(BIND_ANIMATED_DIRECTION),
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
