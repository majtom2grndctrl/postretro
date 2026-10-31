// Lighting section group: lights, SH base volume, lightmaps, SDF, light chunks.
// See: context/lib/build_pipeline.md §PRL Compilation

use postretro_level_format::alpha_lights::{
    AlphaFalloffModel, AlphaLightType, AlphaLightsSection, AlphaShadowType,
};
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::light_influence::LightInfluenceSection;
use postretro_level_format::light_tags::LightTagsSection;
use postretro_level_format::sdf_atlas::SdfAtlasSection;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::{self as prl_format, SectionId};
use postretro_render_data::influence::LightInfluence;

use super::geometry::GeometryDecode;
use super::spatial::SpatialSections;
use crate::lightmap_stream::{
    LightmapResidencyInputs, LightmapResidencyRead, LightmapStreamingMode,
};
use crate::prl_container::PrlContainer;
use crate::prl_lightmap::{LoadedLightmap, LoadedShadowmask};
use crate::sh_stream::ShStreamManifest;
use crate::{FalloffModel, LightType, MapLight, PortalData, PrlLoadError, ShadowType};

pub(crate) fn convert_alpha_lights(section: AlphaLightsSection) -> Vec<MapLight> {
    section
        .lights
        .into_iter()
        .map(|r| {
            let light_type = match r.light_type {
                AlphaLightType::Point => LightType::Point,
                AlphaLightType::Spot => LightType::Spot,
                AlphaLightType::Directional => LightType::Directional,
            };
            let falloff_model = match r.falloff_model {
                AlphaFalloffModel::Linear => FalloffModel::Linear,
                AlphaFalloffModel::InverseDistance => FalloffModel::InverseDistance,
                AlphaFalloffModel::InverseSquared => FalloffModel::InverseSquared,
            };
            let shadow_type = match r.shadow_type {
                AlphaShadowType::StaticLightMap => ShadowType::StaticLightMap,
                AlphaShadowType::Sdf => ShadowType::Sdf,
            };
            MapLight {
                origin: r.origin,
                light_type,
                intensity: r.intensity,
                color: r.color,
                falloff_model,
                falloff_range: r.falloff_range,
                cone_angle_inner: r.cone_angle_inner,
                cone_angle_outer: r.cone_angle_outer,
                cone_direction: r.cone_direction,
                is_dynamic: r.is_dynamic,
                casts_entity_shadows: r.casts_entity_shadows,
                animated_slot: None, // populated from ShVolume slot table later in load
                tags: vec![],        // populated by LightTags section pass below
                cell_index: r.leaf_index,
                shadow_type,
            }
        })
        .collect()
}

pub(super) struct LightingDecode {
    pub(super) lights: Vec<MapLight>,
    pub(super) light_influences: Vec<LightInfluence>,
    pub(super) sh_volume: Option<OctahedralShVolumeSection>,
    pub(super) lightmap: Option<LoadedLightmap>,
    pub(super) lightmap_residency: LightmapResidencyRead,
    pub(super) shadowmask_atlas: Option<LoadedShadowmask>,
    pub(super) sdf_atlas: Option<SdfAtlasSection>,
    pub(super) chunk_light_list: Option<ChunkLightListSection>,
    pub(super) animated_light_chunks: Option<AnimatedLightChunksSection>,
    pub(super) animated_light_weight_maps: Option<AnimatedLightWeightMapsSection>,
}

pub(super) fn decode_lighting(
    container: &PrlContainer,
    streaming: Option<&ShStreamManifest>,
    lightmap_requested: LightmapStreamingMode,
    geometry: &GeometryDecode,
    spatial: &SpatialSections,
    portal_data: Option<&[PortalData]>,
) -> Result<LightingDecode, PrlLoadError> {
    let read_section = |section: SectionId| container.read_section(section as u32);
    let GeometryDecode {
        vertices,
        face_meta,
        ..
    } = geometry;
    let SpatialSections {
        cells,
        cell_residency_set,
        ..
    } = spatial;

    // Optional — older maps fall back to empty with a warning.
    let mut lights: Vec<MapLight> = match read_section(SectionId::AlphaLights)? {
        Some(data) => {
            let section = AlphaLightsSection::from_bytes(&data)?;
            let count = section.lights.len();
            let converted = convert_alpha_lights(section);
            log::info!("[PRL] AlphaLights: {count} lights loaded");
            converted
        }
        None => {
            log::warn!(
                "[PRL] AlphaLights section missing — map predates the lighting foundation milestone; recompile with `prl-build` for lights to appear"
            );
            Vec::new()
        }
    };

    // 1:1 with AlphaLights; count mismatch = format error. Absence = no tags.
    if let Some(data) = read_section(SectionId::LightTags)? {
        let section = LightTagsSection::from_bytes(&data)?;
        if section.tags.len() != lights.len() {
            return Err(PrlLoadError::FormatError(prl_format::FormatError::Io(
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "LightTags count ({}) does not match AlphaLights count ({})",
                        section.tags.len(),
                        lights.len()
                    ),
                ),
            )));
        }
        let mut tagged = 0usize;
        for (light, tag_str) in lights.iter_mut().zip(section.tags) {
            let tag_list: Vec<String> = tag_str.split_whitespace().map(|t| t.to_string()).collect();
            if !tag_list.is_empty() {
                tagged += 1;
                light.tags = tag_list;
            }
        }
        log::info!("[PRL] LightTags: {tagged} tagged lights");
    }

    // Optional — absent/short → missing lights are treated as infinite-bound by
    // downstream consumers. Extra records remain malformed: they cannot map to a
    // light and would hide writer bugs if silently ignored.
    let light_influences: Vec<LightInfluence> = match read_section(SectionId::LightInfluence)? {
        Some(data) => {
            let section = LightInfluenceSection::from_bytes(&data)?;
            if section.records.len() > lights.len() {
                return Err(PrlLoadError::FormatError(prl_format::FormatError::Io(
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "LightInfluence record count ({}) exceeds AlphaLights count ({})",
                            section.records.len(),
                            lights.len()
                        ),
                    ),
                )));
            }
            if section.records.len() < lights.len() {
                log::warn!(
                    "[PRL] LightInfluence: {} records for {} lights; missing tail entries are uncullable",
                    section.records.len(),
                    lights.len()
                );
            }
            let converted: Vec<_> = section
                .records
                .into_iter()
                .map(|r| LightInfluence {
                    center: glam::Vec3::from(r.center),
                    radius: r.radius,
                })
                .collect();
            log::info!("[PRL] LightInfluence: {} records loaded", converted.len());
            converted
        }
        None => {
            log::warn!("[Loader] LightInfluence section missing, no spatial culling this map");
            Vec::new()
        }
    };

    let sh_volume: Option<OctahedralShVolumeSection> = match streaming {
        // The manifest retains this metadata; legacy body fields are reserved
        // for a whole-resident legacy load.
        Some(_) => None,
        None => match read_section(SectionId::OctahedralShVolume)? {
            Some(data) => {
                let section = OctahedralShVolumeSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] OctahedralShVolume: {}×{}×{} grid ({} probes, {}×{} atlas, {} atlas layer(s), tile {} + border {}, {} tile(s)/row, {} tile(s)/layer, {} animated descriptor(s))",
                    section.grid_dimensions[0],
                    section.grid_dimensions[1],
                    section.grid_dimensions[2],
                    section.probes.len(),
                    section.atlas_dimensions[0],
                    section.atlas_dimensions[1],
                    section.layer_count,
                    section.tile_dimension,
                    section.tile_border,
                    section.atlas_tiles_per_row,
                    section.tiles_per_layer,
                    section.animation_descriptors.len(),
                );
                Some(section)
            }
            None => return Err(PrlLoadError::NoOctahedralShVolume),
        },
    };

    // Populate `MapLight.animated_slot` from the SH-volume slot table.
    // Resolution happens once here (load time), not per
    // `setLightAnimation` call. Legacy PRLs lack the table — every slot stays
    // `None` and the bridge takes the legacy `is_dynamic`-gated path.
    let sh_slot_for_map_light = streaming.map_or_else(
        || {
            sh_volume
                .as_ref()
                .map_or(&[][..], |section| section.slot_for_map_light.as_slice())
        },
        |manifest| manifest.base().slot_for_map_light.as_slice(),
    );
    if !sh_slot_for_map_light.is_empty() {
        use postretro_level_format::sh_volume::ANIMATED_SLOT_NONE;
        if sh_slot_for_map_light.len() != lights.len() {
            log::warn!(
                "[PRL] OctahedralShVolume slot_for_map_light count ({}) != AlphaLights count ({}); skipping animated-slot resolution",
                sh_slot_for_map_light.len(),
                lights.len(),
            );
        } else {
            let mut resolved = 0usize;
            for (light, &slot) in lights.iter_mut().zip(sh_slot_for_map_light) {
                if slot != ANIMATED_SLOT_NONE {
                    light.animated_slot = Some(slot);
                    resolved += 1;
                }
            }
            log::info!("[PRL] Resolved {resolved} map-light → animated-slot mapping(s)");
        }
    }

    // Streaming mode reads only the id-22/42 index prefixes; all-resident
    // reads both sections whole.
    let (lightmap, mut lightmap_residency) = crate::lightmap_stream::read_lightmap_for_residency(
        container,
        LightmapResidencyInputs {
            requested: lightmap_requested,
            has_residency_set: cell_residency_set.is_some(),
            has_usable_portals: portal_data.is_some(),
            has_retained_file: container.retained_file().is_some(),
        },
        cells.len(),
    )?;
    let lightmap_index = lightmap.as_ref().map(|lightmap| &lightmap.index);
    crate::prl_lightmap::validate_vertex_lightmap_blocks(vertices, lightmap_index)?;
    // Id 42 validates against id 17 and the static spec lights, never id 40.
    let shadowmask_atlas = lightmap_residency.read_shadowmask(
        container,
        lightmap_index,
        crate::prl_lightmap::shadowmask_owner_bounds(face_meta.len(), &lights),
    )?;

    // Optional — absent → no static-occluder SDF; runtime shadow pass disabled.
    // An empty-geometry section (zero grid dims) is also a valid "no SDF"
    // marker; the renderer collapses it to the same disabled state.
    let sdf_atlas: Option<SdfAtlasSection> = match read_section(SectionId::SdfAtlas)? {
        Some(data) => {
            let section = SdfAtlasSection::from_bytes(&data)?;
            log::info!(
                "[PRL] SdfAtlas: grid={}×{}×{}, voxel_size={:.4}m, brick={} voxels, {} surface bricks",
                section.grid_dims[0],
                section.grid_dims[1],
                section.grid_dims[2],
                section.voxel_size_m,
                section.brick_size_voxels,
                section.surface_brick_count,
            );
            Some(section)
        }
        None => {
            log::info!(
                "[PRL] SdfAtlas section missing — runtime SDF shadow pass disabled (legacy PRL or no SDF bake)"
            );
            None
        }
    };

    // Optional — absent → full spec-buffer scan fallback.
    let chunk_light_list: Option<ChunkLightListSection> =
        match read_section(SectionId::ChunkLightList)? {
            Some(data) => {
                let section = ChunkLightListSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] ChunkLightList: {}×{}×{} grid, {} indices",
                    section.grid_dimensions[0],
                    section.grid_dimensions[1],
                    section.grid_dimensions[2],
                    section.light_indices.len(),
                );
                Some(section)
            }
            None => {
                log::info!(
                    "[PRL] ChunkLightList section missing — specular path uses full-buffer fallback"
                );
                None
            }
        };

    let animated_light_chunks = crate::prl_lightmap::read_animated_light_chunks(container)?;
    let animated_light_weight_maps = crate::prl_lightmap::read_animated_light_weight_maps(
        container,
        lightmap.as_ref().map(|lightmap| &lightmap.index),
        vertices,
    )?;

    Ok(LightingDecode {
        lights,
        light_influences,
        sh_volume,
        lightmap,
        lightmap_residency,
        shadowmask_atlas,
        sdf_atlas,
        chunk_light_list,
        animated_light_chunks,
        animated_light_weight_maps,
    })
}
