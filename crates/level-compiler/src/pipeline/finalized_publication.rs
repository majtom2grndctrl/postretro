//! Final post-bake metadata and PRL publication handoff.
//!
//! This keeps final presence decisions, the cluster-directory inventory, and
//! final serialization adjacent without retaining cloned SH atlas payloads.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use postretro_level_format::alpha_lights::AlphaLightsSection;
use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection;
use postretro_level_format::bsp::BspLeavesSection;
use postretro_level_format::bvh::BvhSection;
use postretro_level_format::cell_draw_index::CellDrawIndexSection;
use postretro_level_format::cell_visibility::CellVisibilitySection;
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::data_script::DataScriptSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::fog_cell_masks::FogCellMasksSection;
use postretro_level_format::fog_volumes::FogVolumesSection;
use postretro_level_format::kinematic_geometry::KinematicGeometrySection;
use postretro_level_format::light_influence::LightInfluenceSection;
use postretro_level_format::light_tags::LightTagsSection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::map_entity::MapEntitySection;
use postretro_level_format::navmesh::NavMeshSection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::sdf_atlas::SdfAtlasSection;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;
use postretro_level_format::trigger_volumes::TriggerVolumesSection;

use super::{pack, portals};
use crate::cluster_directory_bake::ClusterDirectoryBake;
use crate::geometry::GeometryResult;
use crate::map_data::{MapStreamingHintRegion, MapStreamingPriorityRegion};
use crate::partition::BspTree;
use crate::streaming_hints::resolve_streaming_hints;

/// Final post-bake data shared by directory construction and serialization.
///
/// `sh_pack.sources` deliberately borrows the pre-BC6H RGBA16F atlases until
/// the final staged PRL write completes. Legacy sections still serialize the
/// encoded `sh_pack.emission` view.
pub(super) struct FinalizedClusterMetadata<'a> {
    portals: PortalsSection,
    sh_pack: pack::FinalizedShPack<'a>,
    cluster_directory: ClusterDirectoryBake,
}

/// Inputs that determine the final cluster-directory metadata inventory.
pub(super) struct FinalizedClusterMetadataInputs<'a> {
    pub(super) generated_portals: &'a [portals::Portal],
    pub(super) streaming_seam_regions: &'a [MapStreamingHintRegion],
    pub(super) stream_resident_regions: &'a [MapStreamingHintRegion],
    pub(super) stream_priority_regions: &'a [MapStreamingPriorityRegion],
    pub(super) leaves: &'a BspLeavesSection,
    pub(super) tree: &'a BspTree,
    pub(super) exterior_leaves: &'a HashSet<usize>,
    pub(super) bvh: &'a BvhSection,
    pub(super) bvh_chunk_ranges: &'a [(u32, u32)],
    pub(super) packed_sh_volume: &'a OctahedralShVolumeSection,
    pub(super) packed_direct: Option<&'a DirectShVolumeSection>,
    pub(super) sh_volume: &'a OctahedralShVolumeSection,
    pub(super) direct_sh_volume: Option<&'a DirectShVolumeSection>,
    pub(super) delta_sh_volumes: Option<&'a DeltaShVolumesSection>,
    pub(super) entity_shadow_lights: Option<&'a EntityShadowLightsSection>,
    pub(super) direct_sh_delta_volumes: Option<&'a DirectShDeltaVolumesSection>,
    pub(super) animated_direct_sh_delta_volumes: Option<&'a AnimatedDirectShDeltaVolumesSection>,
    pub(super) billboard_direct_scatter_volume: Option<&'a BillboardDirectScatterVolumeSection>,
    pub(super) animated_billboard_direct_scatter_delta_volumes:
        Option<&'a AnimatedBillboardDirectScatterDeltaVolumesSection>,
}

/// Build the finalized SH inventory, derived cell metadata, and cluster
/// directory in the order they are consumed by the final PRL publication.
pub(super) fn build_finalized_cluster_metadata<'a>(
    inputs: FinalizedClusterMetadataInputs<'a>,
) -> anyhow::Result<FinalizedClusterMetadata<'a>> {
    let portals = pack::encode_portals(inputs.generated_portals)?;
    let cells = pack::encode_cells(inputs.leaves, &portals, inputs.exterior_leaves)?;
    let locator = pack::encode_cell_locator(inputs.tree)?;
    let bvh = pack::bvh_with_chunk_ranges(inputs.bvh, inputs.bvh_chunk_ranges);
    let sources = pack::FinalizedShPackSources::new(inputs.packed_sh_volume, inputs.packed_direct)?;
    let emission = pack::FinalizedShEmissionView::new(
        inputs.sh_volume,
        inputs.direct_sh_volume,
        inputs.delta_sh_volumes,
        inputs.entity_shadow_lights,
        inputs.direct_sh_delta_volumes,
        inputs.animated_direct_sh_delta_volumes,
        inputs.billboard_direct_scatter_volume,
        inputs.animated_billboard_direct_scatter_delta_volumes,
    )?;
    let sh_pack = pack::FinalizedShPack::new(emission, sources)?;
    let streaming_hints = resolve_streaming_hints(
        inputs.streaming_seam_regions,
        inputs.stream_resident_regions,
        inputs.stream_priority_regions,
        inputs.generated_portals,
        &portals,
        &cells,
    )?;
    let cluster_directory = crate::cluster_directory_bake::bake_cluster_directory(
        &cells,
        &portals,
        &bvh,
        &locator,
        sh_pack.emission,
        &streaming_hints,
    )?;

    Ok(FinalizedClusterMetadata {
        portals,
        sh_pack,
        cluster_directory,
    })
}

/// Inputs held only through the final staged PRL write.
pub(super) struct FinalizedPrlPackInputs<'a> {
    pub(super) output: &'a Path,
    pub(super) geo_result: &'a GeometryResult,
    pub(super) texture_cache_keys: &'a HashMap<String, [u8; 32]>,
    pub(super) leaves: &'a BspLeavesSection,
    pub(super) tree: &'a BspTree,
    pub(super) exterior_leaves: &'a HashSet<usize>,
    pub(super) bvh: &'a BvhSection,
    pub(super) bvh_chunk_ranges: &'a [(u32, u32)],
    pub(super) alpha_lights: &'a AlphaLightsSection,
    pub(super) light_influence: &'a LightInfluenceSection,
    pub(super) sh_volume: &'a OctahedralShVolumeSection,
    pub(super) direct_sh_volume: Option<&'a DirectShVolumeSection>,
    pub(super) entity_shadow_lights: Option<&'a EntityShadowLightsSection>,
    pub(super) direct_sh_delta_volumes: Option<&'a DirectShDeltaVolumesSection>,
    pub(super) shadowmask_atlas: Option<&'a ShadowmaskAtlasSection>,
    pub(super) lightmap: &'a LightmapSection,
    pub(super) chunk_light_list: &'a ChunkLightListSection,
    pub(super) animated_light_chunks: Option<&'a AnimatedLightChunksSection>,
    pub(super) animated_light_weight_maps: Option<&'a AnimatedLightWeightMapsSection>,
    pub(super) light_tags: Option<&'a LightTagsSection>,
    pub(super) delta_sh_volumes: Option<&'a DeltaShVolumesSection>,
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
    pub(super) animated_direct_sh_delta_volumes: Option<&'a AnimatedDirectShDeltaVolumesSection>,
    pub(super) billboard_direct_scatter_volume: Option<&'a BillboardDirectScatterVolumeSection>,
    pub(super) animated_billboard_direct_scatter_delta_volumes:
        Option<&'a AnimatedBillboardDirectScatterDeltaVolumesSection>,
    pub(super) metadata: FinalizedClusterMetadata<'a>,
}

/// Publish a final PRL using the previously baked metadata and exact SH view.
pub(super) fn write_finalized_prl(inputs: FinalizedPrlPackInputs<'_>) -> anyhow::Result<()> {
    let FinalizedPrlPackInputs {
        output,
        geo_result,
        texture_cache_keys,
        leaves,
        tree,
        exterior_leaves,
        bvh,
        bvh_chunk_ranges,
        alpha_lights,
        light_influence,
        sh_volume,
        direct_sh_volume,
        entity_shadow_lights,
        direct_sh_delta_volumes,
        shadowmask_atlas,
        lightmap,
        chunk_light_list,
        animated_light_chunks,
        animated_light_weight_maps,
        light_tags,
        delta_sh_volumes,
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
        animated_direct_sh_delta_volumes,
        billboard_direct_scatter_volume,
        animated_billboard_direct_scatter_delta_volumes,
        metadata:
            FinalizedClusterMetadata {
                portals,
                sh_pack,
                cluster_directory,
            },
    } = inputs;

    pack::pack_and_write_portals_with_billboard_scatter_finalized(
        output,
        geo_result,
        texture_cache_keys,
        leaves,
        tree,
        &portals,
        exterior_leaves,
        bvh,
        bvh_chunk_ranges,
        alpha_lights,
        light_influence,
        sh_volume,
        direct_sh_volume,
        entity_shadow_lights,
        direct_sh_delta_volumes,
        shadowmask_atlas,
        lightmap,
        chunk_light_list,
        animated_light_chunks,
        animated_light_weight_maps,
        light_tags,
        delta_sh_volumes,
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
        animated_direct_sh_delta_volumes,
        billboard_direct_scatter_volume,
        animated_billboard_direct_scatter_delta_volumes,
        Some((sh_pack, &cluster_directory)),
    )
}
