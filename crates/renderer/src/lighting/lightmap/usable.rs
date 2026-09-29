// Lightmap and shadowmask header usability filters: which sections the device
// can bind, and which headers arrived with their payloads.
// See: context/lib/rendering_pipeline.md §4

use postretro_level_format::lightmap::LightmapHeader;
use postretro_level_format::shadowmask_atlas::{
    SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE, SHADOWMASK_GROUP_COUNT, ShadowmaskAtlasHeader,
    ShadowmaskAtlasSection,
};

/// A usable header with the payload install moved in. A header that arrives
/// without its payload — hand-built geometry with default payloads, or a
/// payload already taken — falls back to the placeholder rather than
/// panicking. A payload whose header was filtered out is dropped here, at the
/// upload that owns it.
pub(super) fn paired_with_payload<'a, H, P>(
    header: Option<&'a H>,
    payload: Option<P>,
    section_name: &str,
) -> Option<(&'a H, P)> {
    match (header, payload) {
        (Some(header), Some(payload)) => Some((header, payload)),
        (Some(_), None) => {
            log::error!(
                "[Renderer] {section_name} header arrived without its payload; using the \
                 neutral placeholder"
            );
            None
        }
        (None, _) => None,
    }
}

/// The static lightmap layer the animated atlas lives beside, as
/// `(layer size, layer count)`, using the same usability filter as `new()`.
/// Returns `None` when the section is absent, zero-area, oversize, non-square,
/// or the 1×1 placeholder: the block table translates both UV axes by one
/// layer size, so without a real square static atlas it has no coordinate
/// space, and the level takes the no-animated-light path. `new()`
/// also falls back to the placeholder when a usable header arrives without its
/// payload, so level installs pass the header only when its payload is
/// present (renderer boot passes no level); the two then fall back together.
pub(crate) fn usable_static_layers(
    section: Option<&LightmapHeader>,
    max_texture_dimension_2d: u32,
    max_texture_array_layers: u32,
) -> Option<(u32, u32)> {
    filter_usable_section(section, max_texture_dimension_2d, max_texture_array_layers)
        .filter(|s| !s.is_placeholder() && s.irr_width == s.irr_height)
        .map(|s| (s.irr_width, s.layer_count))
}

/// Filter out an absent (`None`), invalid, or device-incompatible
/// `LightmapSection`, returning `None` so the caller falls through to the
/// neutral placeholder. Pure dimension-vs-limit comparison — unit-testable
/// without a real wgpu device.
pub(super) fn filter_usable_section(
    section: Option<&LightmapHeader>,
    max_texture_dimension_2d: u32,
    max_texture_array_layers: u32,
) -> Option<&LightmapHeader> {
    section
        .filter(|s| s.irr_width > 0 && s.irr_height > 0)
        .filter(|s| s.dir_width > 0 && s.dir_height > 0)
        .filter(|s| s.layer_count > 0)
        .filter(|s| {
            let fits =
                s.irr_width <= max_texture_dimension_2d && s.irr_height <= max_texture_dimension_2d;
            if !fits {
                log::error!(
                    "[Renderer] Lightmap atlas {}x{} exceeds device maxTextureDimension2D {}; \
                         degrading to neutral placeholder for this level",
                    s.irr_width,
                    s.irr_height,
                    max_texture_dimension_2d,
                );
            }
            fits
        })
        .filter(|s| {
            let fits =
                s.dir_width <= max_texture_dimension_2d && s.dir_height <= max_texture_dimension_2d;
            if !fits {
                log::error!(
                    "[Renderer] Lightmap direction atlas {}x{} exceeds device \
                         maxTextureDimension2D {}; degrading to neutral placeholder for this level",
                    s.dir_width,
                    s.dir_height,
                    max_texture_dimension_2d,
                );
            }
            fits
        })
        .filter(|s| {
            let fits = s.layer_count <= max_texture_array_layers;
            if !fits {
                log::error!(
                    "[Renderer] Lightmap atlas has {} layer(s), exceeding device \
                         maxTextureArrayLayers {}; degrading to neutral placeholder for this level",
                    s.layer_count,
                    max_texture_array_layers,
                );
            }
            fits
        })
}

pub(super) fn filter_usable_shadowmask_section(
    section: Option<&ShadowmaskAtlasHeader>,
    max_texture_dimension_2d: u32,
    max_texture_array_layers: u32,
) -> Option<&ShadowmaskAtlasHeader> {
    section
        .filter(|s| s.width > 0 && s.height > 0 && s.layer_count > 0)
        .filter(|s| {
            // `from_bytes` rejects unknown tags; this guards hand-built
            // sections, since the upload decodes only the one BC5 layout.
            let known = s.format == SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE;
            if !known {
                log::error!(
                    "[Renderer] ShadowmaskAtlas format tag {:#010x} is not BC5 side-by-side; \
                         disabling entity-to-world static-light shadowmask; static world \
                         specular falls back to fully lit for this level",
                    s.format,
                );
            }
            known
        })
        .filter(|s| {
            // The compiler never emits misaligned data and `from_bytes` rejects
            // it; this guards hand-built sections before BC5 texture creation.
            let aligned = s.width % 4 == 0 && s.height % 4 == 0;
            if !aligned {
                log::error!(
                    "[Renderer] ShadowmaskAtlas {}x{} is not BC5 block-aligned; disabling \
                         entity-to-world static-light shadowmask; static world specular falls \
                         back to fully lit for this level",
                    s.width,
                    s.height,
                );
            }
            aligned
        })
        .filter(|s| {
            let texture_width = s.texture_width();
            let fits = texture_width.is_some_and(|w| w <= max_texture_dimension_2d)
                && s.height <= max_texture_dimension_2d;
            if !fits {
                log::error!(
                    "[Renderer] ShadowmaskAtlas texture {}x{} (two {}-wide mask groups) exceeds \
                         device maxTextureDimension2D {}; disabling entity-to-world static-light \
                         shadowmask; static world specular falls back to fully lit for this level",
                    u64::from(s.width) * u64::from(SHADOWMASK_GROUP_COUNT),
                    s.height,
                    s.width,
                    max_texture_dimension_2d,
                );
            }
            fits
        })
        .filter(|s| {
            let fits = s.layer_count <= max_texture_array_layers;
            if !fits {
                log::error!(
                    "[Renderer] ShadowmaskAtlas has {} layer(s), exceeding device \
                         maxTextureArrayLayers {}; disabling entity-to-world static-light \
                         shadowmask; static world specular falls back to fully lit for this level",
                    s.layer_count,
                    max_texture_array_layers,
                );
            }
            fits
        })
}

/// Whether a paired shadowmask payload holds exactly the BC5 bytes its header
/// describes. `from_bytes` enforces this on the wire; hand-built geometry can
/// still pair a header with a payload of another shape, which would fail
/// texture creation, so it degrades to the placeholder instead.
pub(super) fn shadowmask_payload_matches_header(sec: &ShadowmaskAtlasHeader, data: &[u8]) -> bool {
    let expected = ShadowmaskAtlasSection::payload_len(sec.width, sec.height, sec.layer_count);
    let matches = expected == Some(data.len());
    if !matches {
        log::error!(
            "[Renderer] ShadowmaskAtlas payload is {} bytes, expected {:?} for {}x{}x{}; \
                 disabling entity-to-world static-light shadowmask; static world specular \
                 falls back to fully lit for this level",
            data.len(),
            expected,
            sec.width,
            sec.height,
            sec.layer_count,
        );
    }
    matches
}
