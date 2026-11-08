// PRL section decoding and cross-validation for runtime level data.
// See: context/lib/build_pipeline.md §PRL Compilation

mod cell_draw_index;
mod cluster_directory;
mod entities;
mod geometry;
mod lighting;
mod lighting_sh;
mod spatial;
mod staged;
mod validation;

use postretro_level_format::SectionId;
use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_format::reflection_probes::ReflectionProbesSection;

use super::{CellDrawIndex, LevelWorld, PortalData, PrlLoadError};
use crate::lightmap_stream::LightmapStreamingMode;
use crate::prl_container::PrlContainer;
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
pub(crate) use staged::load_prl_staged_from_container;
pub use staged::{
    CompleteHandOuts, EntitiesHandOut, FinalHandOut, FinalLighting, GeometryHandOut, HandOut,
    HandOutCollector, HandOutStage, LoadCancel, ShManifestHandOut, ShManifestMetadata,
    SpatialHandOut, StagedParseOutcome, assemble,
};

use entities::EntityDecode;
use geometry::GeometryDecode;
use lighting::LightingDecode;
use spatial::{SpatialDecode, SpatialSections};
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
///
/// The oracle for [`load_prl_staged_from_container`]: both call the same
/// decode steps, each in its own sequence, and assemble the level apart.
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
    let SpatialDecode {
        sections: spatial,
        validation_inputs,
        cell_portal_refs,
        portal_data,
    } = spatial::decode_spatial(&container, &geometry)?;
    let lighting = lighting::decode_lighting(
        &container,
        streaming,
        lightmap_requested,
        &geometry,
        &spatial,
        portal_data.as_deref(),
    )?;
    let sh = lighting_sh::decode_sh_companions(
        &container,
        streaming,
        max_delta_section_binding_bytes,
        max_scatter_section_bytes,
        &lighting,
    )?;
    let mut entities = entities::decode_entities(
        &container,
        &lighting.lights,
        portal_data.as_ref().map(Vec::len),
    )?;
    // Required when the BVH has leaves; omitted only for empty-BVH maps. Hold
    // the raw bytes until Cells and BVH are both available for cross-validation.
    let cell_draw_index_data = container.read_section(SectionId::CellDrawIndex as u32)?;

    entities.fog_cell_masks = validation::validate_cross_sections(
        &geometry,
        &spatial,
        &validation_inputs,
        &lighting.lights,
        entities.fog_volumes.len(),
        entities.fog_cell_masks.take(),
    )?;
    let cell_draw_index = cell_draw_index::decode_cell_draw_index(
        cell_draw_index_data,
        &spatial.bvh,
        &spatial.cells,
    )?;
    let cluster_directory = cluster_directory::validate_cluster_directory(
        &container,
        streaming,
        parsed_cluster_directory,
        &validation_inputs,
        &lighting.sh_volume,
        &sh,
    )?;
    let sh = validation::finalize_sh_companions(sh);
    let (portals, has_portals) =
        validation::validate_portals(&spatial.cells, &cell_portal_refs, portal_data)?;

    assemble_world(
        &container,
        stream_manifest,
        DecodedLevel {
            geometry,
            spatial,
            cell_portal_refs,
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
    cell_portal_refs: Vec<u32>,
    lighting: LightingDecode,
    sh: RuntimeShCompanions,
    entities: EntityDecode,
    cell_draw_index: Option<CellDrawIndex>,
    cluster_directory: Option<ClusterDirectorySection>,
    portals: Vec<PortalData>,
    has_portals: bool,
}

/// Logs the loaded level's headline counts, once per successful decode.
fn log_loaded(geometry: &GeometryDecode, spatial: &SpatialSections, portal_count: usize) {
    log::info!(
        "[PRL] Loaded: {} vertices, {} indices ({} triangles), {} faces, {} cells, bvh=[{} nodes, {} leaves], portals={}, textures={}",
        geometry.vertices.len(),
        geometry.indices.len(),
        geometry.indices.len() / 3,
        geometry.face_meta.len(),
        spatial.cells.len(),
        spatial.bvh.nodes.len(),
        spatial.bvh.leaves.len(),
        portal_count,
        geometry.texture_names.len(),
    );
}

/// The parse-end lighting step both sequences share: splits GPU lighting
/// payloads out of the decoded lighting (the shadowmask drop), finalizes
/// lightmap storage (the lightmap stream manifest), and stamps the vertex
/// owner stream. Fallible: lightmap storage can still reject the level.
fn finalize_lighting(
    container: &PrlContainer,
    stream_manifest: Option<std::sync::Arc<ShStreamManifest>>,
    lighting: LightingDecode,
    sh: RuntimeShCompanions,
    cluster_directory: Option<ClusterDirectorySection>,
    geometry: &GeometryDecode,
    spatial: &SpatialSections,
) -> Result<FinalLighting, PrlLoadError> {
    let LightingDecode {
        lights,
        light_influences,
        sh_volume,
        probe_depth_tiles,
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

    // Both load modes keep the id-22 header, which carries the bake mode.
    let lightmap_mode = crate::prl_lightmap::lightmap_mode(lightmap.as_ref());
    let (lightmap, shadowmask_atlas, gpu_lighting_payloads) =
        crate::prl_lightmap::split_gpu_lighting(lightmap, shadowmask_atlas);
    let lightmap_storage = lightmap_residency.into_storage(
        container,
        lightmap.as_ref(),
        shadowmask_atlas.as_ref(),
        stream_manifest.as_deref(),
    )?;
    let vertex_spec_owners = crate::prl_spec_owners::build_vertex_spec_owners(
        geometry.vertices.len(),
        &geometry.indices,
        &spatial.bvh.leaves,
        shadowmask_atlas
            .as_ref()
            .map(|index| index.owners.as_slice()),
    );
    Ok(FinalLighting {
        lights,
        light_influences,
        sh_volume,
        sh_storage: match stream_manifest {
            Some(manifest) => ShStorage::Streaming(manifest),
            None => ShStorage::Legacy,
        },
        probe_depth_tiles,
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
        vertex_spec_owners,
        gpu_lighting_payloads,
        lightmap_storage,
        cluster_directory,
    })
}

/// The reflection-probe roster check, the last parse-end check: it reads the
/// finalized lighting.
fn validate_reflection_probes(
    reflection_probes: Option<&ReflectionProbesSection>,
    lighting: &FinalLighting,
) -> Result<(), PrlLoadError> {
    crate::prl_environment::validate_reflection_probe_lights(
        reflection_probes,
        crate::prl_environment::ReflectionProbeRoster {
            sh_manifest: lighting.sh_storage.manifest().map(|manifest| &**manifest),
            sh_volume: lighting.sh_volume.as_ref(),
            delta_sh_volumes: lighting.delta_sh_volumes.as_ref(),
            animated_direct_sh_delta_volumes: lighting.animated_direct_sh_delta_volumes.as_ref(),
        },
    )
}

/// Finalizes the lighting, assembles the `LevelWorld`, and runs the last
/// parse-end check against it.
fn assemble_world(
    container: &PrlContainer,
    stream_manifest: Option<std::sync::Arc<ShStreamManifest>>,
    decoded: DecodedLevel,
) -> Result<LevelWorld, PrlLoadError> {
    let DecodedLevel {
        geometry,
        spatial,
        cell_portal_refs,
        lighting,
        sh,
        entities,
        cell_draw_index,
        cluster_directory,
        portals,
        has_portals,
    } = decoded;

    log_loaded(&geometry, &spatial, portals.len());
    let lighting = finalize_lighting(
        container,
        stream_manifest,
        lighting,
        sh,
        cluster_directory,
        &geometry,
        &spatial,
    )?;
    validate_reflection_probes(entities.reflection_probes.as_ref(), &lighting)?;

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
        cell_visibility,
        cell_residency_set,
        cell_locator_root,
        cell_locator_nodes,
    } = spatial;
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
    let FinalLighting {
        lights,
        light_influences,
        sh_volume,
        sh_storage,
        probe_depth_tiles,
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
        vertex_spec_owners,
        gpu_lighting_payloads,
        lightmap_storage,
        cluster_directory,
    } = lighting;

    Ok(LevelWorld {
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
        lights,
        light_influences,
        sh_volume,
        sh_storage,
        probe_depth_tiles,
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
        cluster_directory,
        cell_residency_set,
        prl_read_counters: container.read_counters().cloned(),
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod staged_tests;
