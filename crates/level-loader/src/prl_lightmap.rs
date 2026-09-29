// Lightmap-family PRL section decoding (ids 22, 42, 24, 25) and the id-22/42
// GPU payload split.
// See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::SectionId;
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::lightmap::{LightmapHeader, LightmapPayloads, LightmapSection};
use postretro_level_format::shadowmask_atlas::{ShadowmaskAtlasHeader, ShadowmaskAtlasSection};
use postretro_render_data::geometry::WorldVertex;

use crate::prl::PrlLoadError;
use crate::prl_container::PrlContainer;

// Optional — absent → 1×1 white placeholder; bumped-Lambert degrades to flat white.
pub(crate) fn read_lightmap(
    container: &PrlContainer,
) -> Result<Option<LightmapSection>, PrlLoadError> {
    Ok(match container.read_section(SectionId::Lightmap as u32)? {
        Some(data) => {
            let section = LightmapSection::from_bytes(&data)?;
            log::info!(
                "[PRL] Lightmap: {}x{} atlas, {} layer(s), {} B irradiance, {} B direction",
                section.irr_width,
                section.irr_height,
                section.layer_count,
                section.irradiance.len(),
                section.direction.len(),
            );
            Some(section)
        }
        None => {
            log::warn!(
                "[PRL] Lightmap section missing — static direct lighting disabled for this map"
            );
            None
        }
    })
}

/// Id 42, kept only when its atlas matches the id-22 irradiance atlas.
pub(crate) fn read_shadowmask_atlas(
    container: &PrlContainer,
    lightmap: Option<&LightmapSection>,
) -> Result<Option<ShadowmaskAtlasSection>, PrlLoadError> {
    Ok(
        match container.read_section(SectionId::ShadowmaskAtlas as u32)? {
            Some(data) => match ShadowmaskAtlasSection::from_bytes(&data) {
                Ok(section) => match lightmap {
                    Some(lm) => {
                        if section.width == lm.irr_width
                            && section.height == lm.irr_height
                            && section.layer_count == lm.layer_count
                        {
                            log::info!(
                                "[PRL] ShadowmaskAtlas: {}x{} atlas, {} layer(s), {} selected channel entr(y/ies), {} payload byte(s)",
                                section.width,
                                section.height,
                                section.layer_count,
                                section.channels.len(),
                                section.data.len(),
                            );
                            Some(section)
                        } else {
                            log::warn!(
                                "[PRL] ShadowmaskAtlas dimensions do not match Lightmap irradiance atlas; ignoring section"
                            );
                            None
                        }
                    }
                    None => {
                        log::warn!(
                            "[PRL] ShadowmaskAtlas present without Lightmap; ignoring SectionId 42"
                        );
                        None
                    }
                },
                Err(err) => {
                    log::warn!("[PRL] ShadowmaskAtlas malformed; ignoring section: {err}");
                    None
                }
            },
            None => None,
        },
    )
}

// Optional — cross-checked against weight-map chunk count at runtime.
pub(crate) fn read_animated_light_chunks(
    container: &PrlContainer,
) -> Result<Option<AnimatedLightChunksSection>, PrlLoadError> {
    Ok(
        match container.read_section(SectionId::AnimatedLightChunks as u32)? {
            Some(data) => {
                let section = AnimatedLightChunksSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] AnimatedLightChunks: {} chunks, {} flat indices",
                    section.chunks.len(),
                    section.light_indices.len(),
                );
                Some(section)
            }
            None => None,
        },
    )
}

// Optional — absent → 1×1 zero atlas on animated-contribution slot.
pub(crate) fn read_animated_light_weight_maps(
    container: &PrlContainer,
    lightmap: Option<&LightmapSection>,
    vertices: &[WorldVertex],
) -> Result<Option<AnimatedLightWeightMapsSection>, PrlLoadError> {
    let animated_light_weight_maps: Option<AnimatedLightWeightMapsSection> =
        match container.read_section(SectionId::AnimatedLightWeightMaps as u32)? {
            Some(data) => {
                let section = AnimatedLightWeightMapsSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] AnimatedLightWeightMaps: {} chunks, {} blocks on {} pages of {}², \
                 {} covered texels, {} weight entries",
                    section.chunk_rects.len(),
                    section.blocks.len(),
                    section.compact_layers,
                    section.page_size,
                    section.offset_counts.len(),
                    section.texel_lights.len(),
                );
                Some(section)
            }
            None => None,
        };
    // Both sections it depends on have decoded: size its pages against the
    // static lightmap layer and cross-check every vertex's block id.
    crate::prl_animated_atlas::check_animated_atlas(animated_light_weight_maps, lightmap, vertices)
}

/// Drop id 42 when its channel table does not match the usable
/// EntityShadowLights selection.
pub(crate) fn reconcile_shadowmask_with_shadow_lights(
    shadowmask_atlas: &mut Option<ShadowmaskAtlasSection>,
    entity_shadow_lights: &[u32],
) {
    if let Some(section) = shadowmask_atlas.as_ref() {
        if entity_shadow_lights.is_empty() {
            log::warn!(
                "[PRL] ShadowmaskAtlas present without usable EntityShadowLights; ignoring section"
            );
            *shadowmask_atlas = None;
        } else if section.channels.len() != entity_shadow_lights.len() {
            log::warn!(
                "[PRL] ShadowmaskAtlas channel table has {} entr(y/ies), but EntityShadowLights has {}; ignoring section",
                section.channels.len(),
                entity_shadow_lights.len(),
            );
            *shadowmask_atlas = None;
        }
    }
}

/// Lightmap (id 22) and shadowmask (id 42) payloads that only the GPU upload
/// reads. A loaded level holds them until install moves them into the upload,
/// which drops them once the textures exist; the level keeps only the headers.
/// An install that uploads nothing leaves them here.
#[derive(Debug, Default, PartialEq)]
pub struct GpuLightingPayloads {
    pub lightmap: Option<LightmapPayloads>,
    pub shadowmask: Option<Vec<u8>>,
}

/// The headers a loaded level keeps for ids 22 and 42, and their payloads.
pub(crate) fn split_gpu_lighting(
    lightmap: Option<LightmapSection>,
    shadowmask_atlas: Option<ShadowmaskAtlasSection>,
) -> (
    Option<LightmapHeader>,
    Option<ShadowmaskAtlasHeader>,
    GpuLightingPayloads,
) {
    let (lightmap_header, lightmap_payloads) = lightmap.map(LightmapSection::into_parts).unzip();
    let (shadowmask_header, shadowmask_payload) = shadowmask_atlas
        .map(ShadowmaskAtlasSection::into_parts)
        .unzip();
    (
        lightmap_header,
        shadowmask_header,
        GpuLightingPayloads {
            lightmap: lightmap_payloads,
            shadowmask: shadowmask_payload,
        },
    )
}
