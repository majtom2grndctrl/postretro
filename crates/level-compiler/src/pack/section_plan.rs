//! Final legacy section descriptors after pack-time presence policy.
//! See: context/lib/build_pipeline.md §PRL Compilation

use super::*;

/// Borrowed inputs for the one-shot legacy PRL descriptor plan.
///
/// This is deliberately assembled only after direct/scatter policy has settled,
/// so metadata stages and serialization share one exact view of what will be
/// emitted. A future cluster payload adds one writer here without changing the
/// legacy section order or re-evaluating presence decisions.
pub(super) struct FinalizedSectionPlanInputs<'a> {
    pub(super) geo_result: &'a GeometryResult,
    pub(super) texture_cache_keys: &'a TextureCacheKeysSection,
    pub(super) cells: &'a CellsSection,
    pub(super) locator: &'a CellLocatorSection,
    pub(super) portals: &'a PortalsSection,
    pub(super) chunk_light_list: &'a ChunkLightListSection,
    pub(super) bvh: &'a BvhSection,
    pub(super) alpha_lights: &'a AlphaLightsSection,
    pub(super) light_influence: &'a LightInfluenceSection,
    pub(super) sh_volume: &'a OctahedralShVolumeSection,
    pub(super) lightmap: &'a LightmapSection,
    pub(super) direct_sh_volume: Option<&'a DirectShVolumeSection>,
    pub(super) entity_shadow_lights: Option<&'a EntityShadowLightsSection>,
    pub(super) direct_sh_delta_volumes: Option<&'a DirectShDeltaVolumesSection>,
    pub(super) shadowmask_atlas: Option<&'a ShadowmaskAtlasSection>,
    pub(super) animated_light_chunks: Option<&'a AnimatedLightChunksSection>,
    pub(super) animated_light_weight_maps: Option<&'a AnimatedLightWeightMapsSection>,
    pub(super) light_tags: Option<&'a LightTagsSection>,
    pub(super) delta_sh_volumes: Option<&'a DeltaShVolumesSection>,
    pub(super) animated_direct_sh_delta_volumes: Option<&'a AnimatedDirectShDeltaVolumesSection>,
    pub(super) billboard_direct_scatter_volume: Option<&'a BillboardDirectScatterVolumeSection>,
    pub(super) animated_billboard_direct_scatter_delta_volumes:
        Option<&'a AnimatedBillboardDirectScatterDeltaVolumesSection>,
    pub(super) data_script: Option<&'a DataScriptSection>,
    pub(super) map_entities: Option<&'a MapEntitySection>,
    pub(super) fog_volumes: &'a FogVolumesSection,
    pub(super) fog_cell_masks: Option<&'a FogCellMasksSection>,
    pub(super) sdf_atlas: Option<&'a SdfAtlasSection>,
    pub(super) navmesh: Option<&'a NavMeshSection>,
    pub(super) kinematic_geometry: Option<&'a KinematicGeometrySection>,
    pub(super) trigger_volumes: Option<&'a TriggerVolumesSection>,
    pub(super) cell_draw_index: Option<&'a CellDrawIndexSection>,
    pub(super) cell_visibility: Option<&'a CellVisibilitySection>,
    pub(super) cluster_bake: &'a crate::cluster_directory_bake::ClusterDirectoryBake,
    pub(super) cluster_payload: Option<super::cluster_sh_payloads::ClusterPayloadSpool>,
}

/// Build the legacy PRL descriptor plan in its established on-disk order.
pub(super) fn build_finalized_section_plan<'a>(
    inputs: FinalizedSectionPlanInputs<'a>,
) -> anyhow::Result<Vec<PlannedSection<'a>>> {
    let FinalizedSectionPlanInputs {
        geo_result,
        texture_cache_keys,
        cells,
        locator,
        portals,
        chunk_light_list,
        bvh,
        alpha_lights,
        light_influence,
        sh_volume,
        lightmap,
        direct_sh_volume,
        entity_shadow_lights,
        direct_sh_delta_volumes,
        shadowmask_atlas,
        animated_light_chunks,
        animated_light_weight_maps,
        light_tags,
        delta_sh_volumes,
        animated_direct_sh_delta_volumes,
        billboard_direct_scatter_volume,
        animated_billboard_direct_scatter_delta_volumes,
        data_script,
        map_entities,
        fog_volumes,
        fog_cell_masks,
        sdf_atlas,
        navmesh,
        kinematic_geometry,
        trigger_volumes,
        cell_draw_index,
        cell_visibility,
        cluster_bake,
        cluster_payload,
    } = inputs;

    let sh_volume_len = sh_volume.try_byte_len().map_err(|error| {
        anyhow::anyhow!("OctahedralShVolume violates its v11 wire contract: {error}")
    })?;
    let has_usable_direct_sh_deltas = direct_sh_delta_volumes.is_some();
    let mut sections = Vec::new();
    sections.push(PlannedSection::new(
        SectionId::Geometry as u32,
        1,
        geo_result.geometry.byte_len(),
        || Ok(geo_result.geometry.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::TextureNames as u32,
        1,
        geo_result.texture_names.byte_len(),
        || Ok(geo_result.texture_names.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::TextureCacheKeys as u32,
        1,
        texture_cache_keys.byte_len(),
        || Ok(texture_cache_keys.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::Cells as u32,
        1,
        cells.byte_len(),
        || Ok(cells.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::CellLocator as u32,
        1,
        locator.byte_len(),
        || Ok(locator.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::Portals as u32,
        1,
        portals.byte_len(),
        || Ok(portals.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::ChunkLightList as u32,
        1,
        chunk_light_list.byte_len(),
        || Ok(chunk_light_list.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::Bvh as u32,
        1,
        bvh.byte_len(),
        || Ok(bvh.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::AlphaLights as u32,
        1,
        alpha_lights.byte_len(),
        || Ok(alpha_lights.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::LightInfluence as u32,
        1,
        light_influence.byte_len(),
        || Ok(light_influence.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::OctahedralShVolume as u32,
        1,
        sh_volume_len,
        || {
            sh_volume.try_to_bytes().map_err(|error| {
                anyhow::anyhow!("OctahedralShVolume violates its v11 wire contract: {error}")
            })
        },
    ));
    sections.push(PlannedSection::new(
        SectionId::Lightmap as u32,
        1,
        lightmap.byte_len(),
        || Ok(lightmap.to_bytes()),
    ));
    if let Some(section) = direct_sh_volume {
        let len = section.try_byte_len().map_err(|error| {
            anyhow::anyhow!("DirectShVolume violates its wire contract: {error}")
        })?;
        sections.push(PlannedSection::new(
            SectionId::DirectShVolume as u32,
            1,
            len,
            || {
                section.try_to_bytes().map_err(|error| {
                    anyhow::anyhow!("DirectShVolume violates its wire contract: {error}")
                })
            },
        ));
    }
    if has_usable_direct_sh_deltas {
        if let Some(section) =
            entity_shadow_lights.filter(|section| !section.light_indices.is_empty())
        {
            sections.push(PlannedSection::new(
                SectionId::EntityShadowLights as u32,
                1,
                section.byte_len(),
                || Ok(section.to_bytes()),
            ));
        }
        if let Some(section) = direct_sh_delta_volumes {
            sections.push(PlannedSection::new(
                SectionId::DirectShDeltaVolumes as u32,
                1,
                section.byte_len(),
                || Ok(section.to_bytes()),
            ));
        }
        if let Some(section) = shadowmask_atlas.filter(|section| !section.channels.is_empty()) {
            sections.push(PlannedSection::new(
                SectionId::ShadowmaskAtlas as u32,
                1,
                section.byte_len(),
                || Ok(section.to_bytes()),
            ));
        }
    }
    if let Some(section) = animated_light_chunks {
        sections.push(PlannedSection::new(
            SectionId::AnimatedLightChunks as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = animated_light_weight_maps {
        sections.push(PlannedSection::new(
            SectionId::AnimatedLightWeightMaps as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = light_tags {
        sections.push(PlannedSection::new(
            SectionId::LightTags as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = delta_sh_volumes {
        sections.push(PlannedSection::new(
            SectionId::DeltaShVolumes as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = animated_direct_sh_delta_volumes {
        let len = section.try_byte_len().map_err(|error| {
            anyhow::anyhow!("AnimatedDirectShDeltaVolumes violates its wire contract: {error}")
        })?;
        sections.push(PlannedSection::new(
            SectionId::AnimatedDirectShDeltaVolumes as u32,
            1,
            len,
            || {
                section.try_to_bytes().map_err(|error| {
                    anyhow::anyhow!(
                        "AnimatedDirectShDeltaVolumes violates its wire contract: {error}"
                    )
                })
            },
        ));
    }
    if let Some(section) = billboard_direct_scatter_volume {
        let len = section.try_byte_len().map_err(|error| {
            anyhow::anyhow!("BillboardDirectScatterVolume violates its wire contract: {error}")
        })?;
        sections.push(PlannedSection::new(
            SectionId::BillboardDirectScatterVolume as u32,
            1,
            len,
            || {
                section.try_to_bytes().map_err(|error| {
                    anyhow::anyhow!(
                        "BillboardDirectScatterVolume violates its wire contract: {error}"
                    )
                })
            },
        ));
    }
    if let Some(section) = animated_billboard_direct_scatter_delta_volumes {
        let len = section.try_byte_len().map_err(|error| {
            anyhow::anyhow!(
                "AnimatedBillboardDirectScatterDeltaVolumes violates its wire contract: {error}"
            )
        })?;
        sections.push(PlannedSection::new(
            SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32,
            1,
            len,
            || {
                section.try_to_bytes().map_err(|error| {
                    anyhow::anyhow!(
                        "AnimatedBillboardDirectScatterDeltaVolumes violates its wire contract: {error}"
                    )
                })
            },
        ));
    }
    if let Some(section) = data_script {
        sections.push(PlannedSection::new(
            SectionId::DataScript as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = map_entities {
        sections.push(PlannedSection::new(
            SectionId::MapEntity as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    sections.push(PlannedSection::new(
        SectionId::FogVolumes as u32,
        1,
        fog_volumes.byte_len(),
        || Ok(fog_volumes.to_bytes()),
    ));
    if let Some(section) = fog_cell_masks {
        sections.push(PlannedSection::new(
            SectionId::FogCellMasks as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = sdf_atlas {
        sections.push(PlannedSection::new(
            SectionId::SdfAtlas as u32,
            postretro_level_format::sdf_atlas::SDF_ATLAS_VERSION as u16,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = navmesh {
        sections.push(PlannedSection::new(
            SectionId::NavMesh as u32,
            NAVMESH_CONTAINER_VERSION,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = kinematic_geometry {
        sections.push(PlannedSection::new(
            SectionId::KinematicGeometry as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = trigger_volumes {
        sections.push(PlannedSection::new(
            SectionId::TriggerVolumes as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = cell_draw_index {
        sections.push(PlannedSection::new(
            SectionId::CellDrawIndex as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = cell_visibility {
        let len = section.try_byte_len().map_err(|error| {
            anyhow::anyhow!("CellVisibility violates its wire contract: {error}")
        })?;
        sections.push(PlannedSection::new(
            SectionId::CellVisibility as u32,
            1,
            len,
            || {
                section.to_bytes().map_err(|error| {
                    anyhow::anyhow!("CellVisibility violates its wire contract: {error}")
                })
            },
        ));
    }
    let cluster_directory_len = cluster_bake.directory.byte_len()?;
    sections.push(PlannedSection::new(
        SectionId::ClusterDirectory as u32,
        CLUSTER_DIRECTORY_CONTAINER_VERSION,
        cluster_directory_len,
        || {
            cluster_bake.directory.try_to_bytes().map_err(|error| {
                anyhow::anyhow!("ClusterDirectory violates its wire contract: {error}")
            })
        },
    ));
    if let Some(cluster_payload) = cluster_payload {
        sections.push(cluster_payload.into_planned_section()?);
    }

    Ok(sections)
}
