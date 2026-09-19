// Material sampler and bind-group helpers.
// See: context/lib/resource_management.md

use super::*;
use postretro_render_cpu::material_plan::{build_material_uniform, mip_lod_max_clamp};
use postretro_render_cpu::surface_depth::{SURFACE_DEPTH_RESIDENT_BASE_MIP, SurfaceDepthUniform};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MipSamplerFiltering {
    mag_filter: wgpu::FilterMode,
    min_filter: wgpu::FilterMode,
    mipmap_filter: wgpu::MipmapFilterMode,
    anisotropy_clamp: u16,
}

const ANISO_MIP_SAMPLER_FILTERING: MipSamplerFiltering = MipSamplerFiltering {
    mag_filter: wgpu::FilterMode::Linear,
    min_filter: wgpu::FilterMode::Linear,
    mipmap_filter: wgpu::MipmapFilterMode::Linear,
    anisotropy_clamp: POST_RETRO_ANISO_CLAMP,
};

const CHARACTER_MODEL_MIP_SAMPLER_FILTERING: MipSamplerFiltering = MipSamplerFiltering {
    mag_filter: wgpu::FilterMode::Nearest,
    min_filter: wgpu::FilterMode::Linear,
    mipmap_filter: wgpu::MipmapFilterMode::Linear,
    anisotropy_clamp: 1,
};

/// Create the Post Retro filtering pool's sampler: fully Linear min/mag/mip
/// with `anisotropy_clamp = POST_RETRO_ANISO_CLAMP`, with a per-mip-count LOD
/// clamp. wgpu 29 validates that aniso > 1 requires all three filters to be
/// Linear. One sampler per distinct mip count is kept in
/// `Renderer::mip_count_aniso_samplers` so world and mover materials bind the
/// clamp that matches their uploaded mip chain. Bound at material binding 5.
pub(crate) fn create_mip_aniso_sampler(device: &wgpu::Device, mip_count: u32) -> wgpu::Sampler {
    create_mip_sampler(
        device,
        mip_count,
        "Mip Texture Aniso Sampler",
        ANISO_MIP_SAMPLER_FILTERING,
    )
}

/// Create a character-model sampler with crisp texels when magnified and
/// linearly filtered mip levels when minified. Anisotropy is disabled because
/// wgpu requires all filter modes to be Linear when it is enabled.
pub(crate) fn create_mip_character_model_sampler(
    device: &wgpu::Device,
    mip_count: u32,
) -> wgpu::Sampler {
    create_mip_sampler(
        device,
        mip_count,
        "Mip Character Model Sampler",
        CHARACTER_MODEL_MIP_SAMPLER_FILTERING,
    )
}

fn create_mip_sampler(
    device: &wgpu::Device,
    mip_count: u32,
    label: &'static str,
    filtering: MipSamplerFiltering,
) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some(label),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: filtering.mag_filter,
        min_filter: filtering.min_filter,
        mipmap_filter: filtering.mipmap_filter,
        lod_min_clamp: 0.0,
        lod_max_clamp: mip_lod_max_clamp(mip_count),
        anisotropy_clamp: filtering.anisotropy_clamp,
        ..Default::default()
    })
}

/// Whether a loaded specular slot carries Surface Depth's second channel.
///
/// `Rg8Unorm` is the surface map the level compiler bakes when a material has
/// an `_h.png` sibling (R = specular, G = depth below the surface). Every other
/// legal specular format is single-channel, and WGSL expands those to
/// `(r, 0, 0, 1)` — `.g == 0`, i.e. flat — so the flag is belt-and-braces over
/// a degradation that is already a no-op. It exists so a material without a
/// height map skips the march instead of paying for an all-zero one.
pub(crate) fn specular_slot_is_surface_map(format: wgpu::TextureFormat) -> bool {
    matches!(format, wgpu::TextureFormat::Rg8Unorm)
}

pub(crate) fn build_material_bind_group(
    device: &wgpu::Device,
    texture_bind_group_layout: &wgpu::BindGroupLayout,
    loaded: &LoadedTexture,
    material_sampler: &wgpu::Sampler,
    material: Material,
    label_prefix: &str,
) -> wgpu::BindGroup {
    // Surface Depth's has-depth flag is decided HERE, from what actually
    // loaded, not from the material prefix: only a two-channel `Rg8Unorm`
    // specular slot is a surface map. A prefix that wants depth but whose
    // `.prm` has no `_h.png` sibling binds the single-channel specular (or the
    // 1x1 black placeholder) and must skip the march entirely rather than walk
    // an all-zero field. The base mip is clamped to the slot's own uploaded
    // chain so the shader's `textureLoad` level can never go out of range.
    let surface_depth = SurfaceDepthUniform::resolve(
        material.surface_depth(),
        specular_slot_is_surface_map(loaded.specular_texture.format()),
        loaded.specular_texture.mip_level_count(),
        SURFACE_DEPTH_RESIDENT_BASE_MIP,
    );
    let uniform_bytes = build_material_uniform(
        material.shininess(),
        material.emissive_strength(),
        surface_depth,
    );
    let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&format!("{label_prefix} Uniform")),
        contents: &uniform_bytes,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&format!("{label_prefix} Bind Group")),
        layout: texture_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&loaded.diffuse_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&loaded.emissive_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&loaded.specular_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&loaded.normal_view),
            },
            // Each draw binds its material-specific sampler at this shared
            // slot: world materials use the Post Retro anisotropic sampler,
            // while skinned models use their nearest-magnification sampler.
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(material_sampler),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_channel_surface_map_sets_the_has_depth_flag() {
        assert!(specular_slot_is_surface_map(wgpu::TextureFormat::Rg8Unorm));
        for other in [
            wgpu::TextureFormat::R8Unorm,
            wgpu::TextureFormat::Bc4RUnorm,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Bc5RgUnorm,
        ] {
            assert!(
                !specular_slot_is_surface_map(other),
                "{other:?} is not a Surface Depth surface map"
            );
        }
    }

    #[test]
    fn character_model_sampler_uses_nearest_magnification_and_linear_minification() {
        assert_eq!(
            CHARACTER_MODEL_MIP_SAMPLER_FILTERING,
            MipSamplerFiltering {
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                anisotropy_clamp: 1,
            }
        );
    }

    #[test]
    fn character_model_sampler_filtering_is_distinct_from_world_anisotropic_filtering() {
        assert_ne!(
            CHARACTER_MODEL_MIP_SAMPLER_FILTERING,
            ANISO_MIP_SAMPLER_FILTERING
        );
    }
}
