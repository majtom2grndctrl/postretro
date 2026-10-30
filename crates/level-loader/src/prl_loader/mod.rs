// PRL section decoding and cross-validation for runtime level data.
// See: context/lib/build_pipeline.md §PRL Compilation

mod cell_draw_index;
mod cluster_directory;
mod entities;
mod geometry;
mod lighting;
mod lighting_sh;
mod spatial;
mod validation;

use postretro_level_format::SectionId;
use postretro_level_format::cluster_directory::ClusterDirectorySection;

use super::{CellDrawIndex, LevelWorld, PortalData, PrlLoadError};
use crate::lightmap_stream::LightmapStreamingMode;
use crate::prl_container::PrlContainer;
use crate::prl_lighting::LoadedLighting;
#[cfg(test)]
use crate::prl_lighting::read_bounded_delta_section_data;
pub(crate) use crate::prl_lighting::{
    BoundedDeltaSectionData, BoundedScatterSectionData, delta_grid_matches_base,
    read_bounded_delta_section_data_with_limit, read_bounded_scatter_section_data_with_limit,
    read_soft_optional_scatter_section_data,
    validate_animated_billboard_direct_scatter_against_metadata,
    validate_animated_billboard_direct_scatter_delta_volumes, validate_animated_direct_sh_delta,
    validate_billboard_direct_scatter_against_metadata, validate_billboard_direct_scatter_volume,
    validate_delta_sh, validate_direct_sh_delta, validate_direct_sh_layout,
    validate_entity_shadow_light_selection, validate_storage_ceiling_for_delta,
};
#[cfg(test)]
pub(crate) use crate::prl_lighting::{expected_affinity_dims, valid_probe_mask_for_affinity_cell};
#[cfg(test)]
use crate::prl_streaming::load_prl;
#[cfg(test)]
pub(crate) use crate::prl_streaming::{
    load_prl_with_delta_binding_limit, load_prl_with_scatter_pack_limit,
    load_prl_with_streaming_mode_for_test,
};
#[cfg(test)]
use crate::sh_stream::ShStreamingMode;
use crate::sh_stream::{ShStorage, ShStreamManifest};

#[cfg(test)]
pub(crate) use cell_draw_index::validate_cell_draw_index;
#[cfg(test)]
pub(crate) use lighting::convert_alpha_lights;
pub(crate) use spatial::{
    convert_cell_locator_section, convert_cells_section, convert_usable_portals,
};

use entities::EntityDecode;
use geometry::GeometryDecode;
use lighting::LightingDecode;
use spatial::SpatialSections;
use validation::RuntimeShCompanions;

/// Conservative desktop floor for one sparse SH delta section bound as a
/// storage buffer. This is intentionally a loader policy rather than a wire
/// limit: the compiler's separate aggregate cap protects bake output.
pub(crate) const MAX_DELTA_SECTION_BINDING_BYTES: u64 = 128 * 1024 * 1024;

fn stale_section(section: &'static str, id: SectionId) -> PrlLoadError {
    PrlLoadError::StaleFormatMissingSection {
        section,
        id: id as u32,
    }
}

fn ambiguous_runtime_bsp_sections(sections: String) -> PrlLoadError {
    PrlLoadError::AmbiguousRuntimeBspSections { sections }
}

pub(crate) fn section_validation(
    section: &'static str,
    message: impl Into<String>,
) -> PrlLoadError {
    PrlLoadError::SectionValidation {
        section,
        message: message.into(),
    }
}

pub(crate) fn section_validation_from_error(
    section: &'static str,
    err: impl std::fmt::Display,
) -> PrlLoadError {
    section_validation(section, err.to_string())
}

/// Decodes every section in file-read order, then runs the parse-end checks.
/// Each section read credits load progress, so the call order here is the
/// progress order; the late checks can still reject after every read.
pub(crate) fn load_prl_from_container(
    container: PrlContainer,
    max_delta_section_binding_bytes: u64,
    max_scatter_section_bytes: u64,
    stream_manifest: Option<std::sync::Arc<ShStreamManifest>>,
    lightmap_requested: LightmapStreamingMode,
) -> Result<LevelWorld, PrlLoadError> {
    let streaming = stream_manifest.as_deref();

    let parsed_cluster_directory =
        cluster_directory::read_cluster_directory_structure(&container, streaming)?;
    let geometry = geometry::decode_geometry(&container)?;
    let spatial = spatial::decode_spatial(&container, &geometry)?;
    let lighting = lighting::decode_lighting(
        &container,
        streaming,
        lightmap_requested,
        &geometry,
        &spatial,
    )?;
    let sh = lighting_sh::decode_sh_companions(
        &container,
        streaming,
        max_delta_section_binding_bytes,
        max_scatter_section_bytes,
        &lighting,
    )?;
    let mut entities = entities::decode_entities(&container, &lighting.lights)?;
    // Required when the BVH has leaves; omitted only for empty-BVH maps. Hold
    // the raw bytes until Cells and BVH are both available for cross-validation.
    let cell_draw_index_data = container.read_section(SectionId::CellDrawIndex as u32)?;

    validation::validate_cross_sections(&geometry, &spatial, &lighting.lights, &mut entities)?;
    let cell_draw_index = cell_draw_index::decode_cell_draw_index(
        cell_draw_index_data,
        &spatial.sections.bvh,
        &spatial.sections.cells,
    )?;
    let cluster_directory = cluster_directory::validate_cluster_directory(
        &container,
        streaming,
        parsed_cluster_directory,
        &spatial.validation_inputs,
        &lighting.sh_volume,
        &sh,
    )?;
    let sh = validation::finalize_sh_companions(sh);
    let (portals, has_portals) = validation::validate_portals(
        &spatial.sections,
        spatial.portal_data,
        &mut entities.kinematic_geometry,
    )?;

    assemble_world(
        &container,
        stream_manifest,
        DecodedLevel {
            geometry,
            spatial: spatial.sections,
            lighting,
            sh,
            entities,
            cell_draw_index,
            cluster_directory,
            portals,
            has_portals,
        },
    )
}

/// Every section group after parse-end validation.
struct DecodedLevel {
    geometry: GeometryDecode,
    spatial: SpatialSections,
    lighting: LightingDecode,
    sh: RuntimeShCompanions,
    entities: EntityDecode,
    cell_draw_index: Option<CellDrawIndex>,
    cluster_directory: Option<ClusterDirectorySection>,
    portals: Vec<PortalData>,
    has_portals: bool,
}

/// Splits GPU lighting payloads out of the decoded lighting, finalizes
/// lightmap storage, and assembles the `LevelWorld`.
fn assemble_world(
    container: &PrlContainer,
    stream_manifest: Option<std::sync::Arc<ShStreamManifest>>,
    decoded: DecodedLevel,
) -> Result<LevelWorld, PrlLoadError> {
    let DecodedLevel {
        geometry,
        spatial,
        lighting,
        sh,
        entities,
        cell_draw_index,
        cluster_directory,
        portals,
        has_portals,
    } = decoded;
    let GeometryDecode {
        vertices,
        indices,
        face_meta,
        texture_names,
        texture_cache_keys,
    } = geometry;
    let SpatialSections {
        bvh,
        cells,
        cell_portal_refs,
        cell_visibility,
        cell_residency_set,
        cell_locator_root,
        cell_locator_nodes,
    } = spatial;
    let LightingDecode {
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
    } = lighting;
    let RuntimeShCompanions {
        delta_sh_volumes,
        direct_sh_volume,
        direct_sh_delta_volumes,
        animated_direct_sh_delta_volumes,
        billboard_direct_scatter_volume,
        animated_billboard_direct_scatter_delta_volumes,
        entity_shadow_lights,
    } = sh;
    let EntityDecode {
        data_script,
        map_entities,
        kinematic_geometry,
        trigger_volumes,
        teleporters,
        ghost_links,
        environment_volumes,
        environment_push_forces,
        reflection_probes,
        fog_volumes,
        fog_pixel_scale,
        fog_cell_masks,
        navmesh,
    } = entities;

    // Both load modes keep the id-22 header, which carries the bake mode.
    let lightmap_mode = crate::prl_lightmap::lightmap_mode(lightmap.as_ref());
    let lighting = LoadedLighting {
        lights,
        light_influences,
        sh_volume,
        lightmap,
        lightmap_mode,
        sdf_atlas,
        chunk_light_list,
        animated_light_chunks,
        animated_light_weight_maps,
        delta_sh_volumes,
        direct_sh_volume,
        direct_sh_delta_volumes,
        animated_direct_sh_delta_volumes,
        billboard_direct_scatter_volume,
        animated_billboard_direct_scatter_delta_volumes,
        entity_shadow_lights,
        shadowmask_atlas,
        cluster_directory,
    };

    log::info!(
        "[PRL] Loaded: {} vertices, {} indices ({} triangles), {} faces, {} cells, bvh=[{} nodes, {} leaves], portals={}, textures={}",
        vertices.len(),
        indices.len(),
        indices.len() / 3,
        face_meta.len(),
        cells.len(),
        bvh.nodes.len(),
        bvh.leaves.len(),
        portals.len(),
        texture_names.len(),
    );

    let (lightmap, shadowmask_atlas, gpu_lighting_payloads) =
        crate::prl_lightmap::split_gpu_lighting(lighting.lightmap, lighting.shadowmask_atlas);
    let lightmap_storage = lightmap_residency.into_storage(
        container,
        lightmap.as_ref(),
        shadowmask_atlas.as_ref(),
        stream_manifest.as_deref(),
    )?;
    let vertex_spec_owners = crate::prl_spec_owners::build_vertex_spec_owners(
        vertices.len(),
        &indices,
        &bvh.leaves,
        shadowmask_atlas
            .as_ref()
            .map(|index| index.owners.as_slice()),
    );
    let prl_read_counters = container.read_counters().cloned();
    let world = LevelWorld {
        vertices,
        indices,
        face_meta,
        cells,
        cell_portal_refs,
        cell_locator_root,
        cell_locator_nodes,
        portals,
        has_portals,
        portal_planes: Default::default(),
        cell_visibility,
        texture_names,
        texture_cache_keys,
        bvh,
        lights: lighting.lights,
        light_influences: lighting.light_influences,
        sh_volume: lighting.sh_volume,
        sh_storage: match stream_manifest {
            Some(manifest) => ShStorage::Streaming(manifest),
            None => ShStorage::Legacy,
        },
        lightmap,
        lightmap_mode: lighting.lightmap_mode,
        sdf_atlas: lighting.sdf_atlas,
        chunk_light_list: lighting.chunk_light_list,
        animated_light_chunks: lighting.animated_light_chunks,
        animated_light_weight_maps: lighting.animated_light_weight_maps,
        delta_sh_volumes: lighting.delta_sh_volumes,
        direct_sh_volume: lighting.direct_sh_volume,
        direct_sh_delta_volumes: lighting.direct_sh_delta_volumes,
        animated_direct_sh_delta_volumes: lighting.animated_direct_sh_delta_volumes,
        billboard_direct_scatter_volume: lighting.billboard_direct_scatter_volume,
        animated_billboard_direct_scatter_delta_volumes: lighting
            .animated_billboard_direct_scatter_delta_volumes,
        entity_shadow_lights: lighting.entity_shadow_lights,
        shadowmask_atlas,
        vertex_spec_owners,
        gpu_lighting_payloads,
        lightmap_storage,
        data_script,
        map_entities,
        kinematic_geometry,
        trigger_volumes,
        teleporters,
        ghost_links,
        fog_volumes,
        fog_pixel_scale,
        environment_volumes,
        environment_push_forces,
        reflection_probes,
        fog_cell_masks,
        navmesh,
        cell_draw_index,
        cluster_directory: lighting.cluster_directory,
        cell_residency_set,
        prl_read_counters,
    };
    crate::prl_environment::validate_reflection_probe_lights(&world)?;
    Ok(world)
}

#[cfg(test)]
mod tests;
