// Directional lightmap GPU resources: irradiance + direction atlas upload,
// sampler, and bind group (group 4).
// See: context/lib/rendering_pipeline.md §4

mod bindings;
#[cfg(test)]
mod tests;
mod upload;
mod usable;

use postretro_level_format::SectionId;
use postretro_level_format::lightmap::LightmapHeader;
use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasHeader;
use wgpu::util::DeviceExt;

use crate::render::residency::{ResidencyAllocation, ResidencyAllocationState, texture_row};
use crate::render::{LIGHTMAP_SHADOWMASK, LIGHTMAP_STATIC_DIRECTION, LIGHTMAP_STATIC_IRRADIANCE};

use bindings::{
    ANIMATED_BLOCK_TABLE_BYTES, BIND_ANIMATED_ATLAS, BIND_ANIMATED_BLOCK_TABLE,
    BIND_ANIMATED_DIRECTION, BIND_DIRECTION, BIND_FILTERING_SAMPLER, BIND_IRRADIANCE, BIND_SAMPLER,
    BIND_SHADOWMASK_ATLAS,
};
pub(crate) use bindings::{
    animated_block_table_bytes, bind_group_layout_entries, filtering_sampler_descriptor,
};
pub use upload::{atlas_format_filterable, bc6h_irradiance_filterable};
use upload::{
    upload_direction_texture, upload_irradiance_texture, upload_placeholder_direction,
    upload_placeholder_irradiance,
};
pub(crate) use upload::{upload_placeholder_shadowmask, upload_shadowmask_texture};
pub(crate) use usable::usable_static_layers;
use usable::{
    filter_usable_section, filter_usable_shadowmask_section, paired_with_payload,
    shadowmask_payload_matches_header,
};

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
    /// Whether a real ShadowmaskAtlas was uploaded (false = 2x1x1 fully-visible
    /// placeholder). Rejected or absent shadowmask data uses this all-visible
    /// fallback so static specular remains fully lit.
    pub shadowmask_present: bool,
    /// Static dominant-direction atlas texture (Rg8Unorm for current sections,
    /// Rgba8Unorm for accepted legacy sections; octahedral in rg).
    /// Forward shading samples it for bumped-Lambert normal-map correction.
    #[allow(dead_code)]
    direction_texture: wgpu::Texture,
    /// Static irradiance, static direction and shadowmask meter rows, read
    /// from the textures this set actually binds.
    pub residency: [ResidencyAllocation; 3],
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
    /// Builds one coherent group-4 resource set. Its independently constructed
    /// static, animated, and layout inputs are intentionally kept explicit at
    /// this renderer boundary rather than wrapped in a one-use parameter type.
    ///
    /// The upload owns the GPU-only payloads and drops them once the textures
    /// exist; the level keeps only the headers.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        section: Option<&LightmapHeader>,
        shadowmask_section: Option<&ShadowmaskAtlasHeader>,
        payloads: postretro_level_loader::GpuLightingPayloads,
        bind_group_layout: &wgpu::BindGroupLayout,
        animated_atlas_view: &wgpu::TextureView,
        animated_direction_view: &wgpu::TextureView,
        animated_block_table: &[u8],
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
        // Rgba16Float) and the BC5 shadowmask. Turns baked penumbra ramps into
        // continuous gradients under magnification. Always used — Rgba16Float
        // linear-filterability is a hard runtime requirement; non-filterable
        // adapters are rejected at init (see `atlas_format_filterable`). See
        // rendering_pipeline.md §4.
        let filtering_sampler = device.create_sampler(&filtering_sampler_descriptor());

        // Defensive runtime guard: the init adapter pre-check guarantees the
        // device grants at least 8192² and at least 256 array layers (see
        // `render::renderer_init_resources.rs`), and the bake's
        // `MAX_ATLAS_DIMENSION` matches that ceiling. A baked atlas larger than
        // the granted limit — or with more layers than the device allows — can
        // only come from future content or a corrupt section. Drop to the
        // neutral placeholder with a logged error rather than panicking on
        // texture creation. Mirrors `render::sh_volume`'s atlas-fits-device filter.
        let limits = device.limits();
        let usable = filter_usable_section(
            section,
            limits.max_texture_dimension_2d,
            limits.max_texture_array_layers,
        );
        let postretro_level_loader::GpuLightingPayloads {
            lightmap: lightmap_payloads,
            shadowmask: shadowmask_payload,
        } = payloads;
        let usable = paired_with_payload(usable, lightmap_payloads, "Lightmap");
        let present = usable.is_some();

        let (irradiance_tex, direction_tex) = match usable {
            Some((sec, payloads)) => (
                upload_irradiance_texture(device, queue, sec, &payloads.irradiance),
                upload_direction_texture(device, queue, sec, &payloads.direction),
            ),
            None => (
                upload_placeholder_irradiance(device, queue),
                upload_placeholder_direction(device, queue),
            ),
        };
        let usable_shadowmask = filter_usable_shadowmask_section(
            shadowmask_section,
            limits.max_texture_dimension_2d,
            limits.max_texture_array_layers,
        );
        let usable_shadowmask =
            paired_with_payload(usable_shadowmask, shadowmask_payload, "ShadowmaskAtlas")
                .filter(|(sec, data)| shadowmask_payload_matches_header(sec, data));
        let shadowmask_present = usable_shadowmask.is_some();
        let shadowmask_tex = match usable_shadowmask {
            Some((sec, data)) => upload_shadowmask_texture(device, queue, sec, &data),
            None => upload_placeholder_shadowmask(device, queue),
        };
        let lightmap_state = residency_state(section.is_some(), present);
        let lightmap_sources = section_source(SectionId::Lightmap, section.is_some());
        let shadowmask_sources =
            section_source(SectionId::ShadowmaskAtlas, shadowmask_section.is_some());
        let residency = [
            texture_row(
                LIGHTMAP_STATIC_IRRADIANCE,
                &irradiance_tex,
                &lightmap_sources,
                lightmap_state,
            ),
            texture_row(
                LIGHTMAP_STATIC_DIRECTION,
                &direction_tex,
                &lightmap_sources,
                lightmap_state,
            ),
            texture_row(
                LIGHTMAP_SHADOWMASK,
                &shadowmask_tex,
                &shadowmask_sources,
                residency_state(shadowmask_section.is_some(), shadowmask_present),
            ),
        ];

        // The irradiance + direction atlases are `texture_2d_array` (group-4
        // bindings 0/1 declare `D2Array`), so their views must declare the same
        // dimension explicitly — the default view dimension follows the texture's
        // layer count, but pinning it keeps the view contract aligned with the BGL.
        let irr_view = irradiance_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let dir_view = direction_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let shadowmask_view = shadowmask_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        debug_assert_eq!(animated_block_table.len(), ANIMATED_BLOCK_TABLE_BYTES);
        let animated_block_table_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Animated LM Block Table"),
                contents: animated_block_table,
                usage: wgpu::BufferUsages::UNIFORM,
            });

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
                wgpu::BindGroupEntry {
                    binding: BIND_SHADOWMASK_ATLAS,
                    resource: wgpu::BindingResource::TextureView(&shadowmask_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_ANIMATED_BLOCK_TABLE,
                    resource: animated_block_table_buffer.as_entire_binding(),
                },
            ],
        });

        Self {
            bind_group,
            present,
            shadowmask_present,
            direction_texture: direction_tex,
            residency,
        }
    }
}

/// Data when the section's texture is bound, Dummy when the section is
/// absent, Fallback when a present section was rejected for its placeholder.
fn residency_state(section_present: bool, texture_present: bool) -> ResidencyAllocationState {
    match (section_present, texture_present) {
        (_, true) => ResidencyAllocationState::Data,
        (true, false) => ResidencyAllocationState::Fallback,
        (false, false) => ResidencyAllocationState::Dummy,
    }
}

/// A row cites its section whenever the level supplied it, as the SH ledger's
/// rows do — including a Fallback row whose section was rejected.
fn section_source(section: SectionId, section_present: bool) -> Vec<u16> {
    if section_present {
        vec![section as u16]
    } else {
        Vec::new()
    }
}
