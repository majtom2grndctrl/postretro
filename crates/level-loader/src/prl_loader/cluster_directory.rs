// ClusterDirectory (id 49): structural read, semantic validation, SH portal topology.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::collections::BTreeSet;

use postretro_level_format::SectionId;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::cluster_directory::{
    CLUSTER_DIRECTORY_CONTAINER_VERSION, ClusterDirectoryError, ClusterDirectorySection,
    ClusterDirectoryShInventory, ClusterDirectoryValidationInputs,
};
use postretro_level_format::delta_sh_volumes::{AFFINITY_FACTOR, DeltaShVolumesSection};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::mover_probe_channel::MoverProbeChannelSection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::lighting_sh::ShCompanions;
use super::section_validation;
use super::spatial::SpatialValidationInputs;
use crate::PrlLoadError;
use crate::prl_container::PrlContainer;
use crate::sh_stream::{ShStreamManifest, ShStreamSeamPortal};

fn cluster_portal_topology_from_portals(
    directory: &ClusterDirectorySection,
    portals: &PortalsSection,
) -> Result<(Vec<Vec<u32>>, Vec<ShStreamSeamPortal>), PrlLoadError> {
    let runtime_cell_count = usize::try_from(directory.runtime_cell_count).map_err(|_| {
        section_validation(
            "ClusterDirectory",
            "runtime cell count exceeds this platform's address space",
        )
    })?;
    let mut cell_to_cluster = vec![None; runtime_cell_count];
    for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
        let member_start = usize::try_from(cluster.member_start)
            .map_err(|_| section_validation("ClusterDirectory", "member start exceeds usize"))?;
        let member_end = member_start
            .checked_add(usize::try_from(cluster.member_count).map_err(|_| {
                section_validation("ClusterDirectory", "member count exceeds usize")
            })?)
            .ok_or_else(|| section_validation("ClusterDirectory", "member range overflows"))?;
        let members = directory
            .members
            .get(member_start..member_end)
            .ok_or_else(|| {
                section_validation(
                    "ClusterDirectory",
                    "validated directory member range is unavailable",
                )
            })?;
        for &cell_id in members {
            let Some(slot) = cell_to_cluster.get_mut(cell_id as usize) else {
                return Err(section_validation(
                    "ClusterDirectory",
                    "validated directory member exceeds runtime cell count",
                ));
            };
            *slot =
                Some(u32::try_from(cluster_id).map_err(|_| {
                    section_validation("ClusterDirectory", "cluster id exceeds u32")
                })?);
        }
    }
    let mut adjacency = vec![BTreeSet::new(); directory.clusters.len()];
    let mut seam_portals = Vec::with_capacity(directory.seam_portal_ids.len());
    for (portal_id, portal) in portals.portals.iter().enumerate() {
        let front = cell_to_cluster
            .get(portal.front_leaf as usize)
            .and_then(|cluster| *cluster)
            .ok_or_else(|| {
                section_validation(
                    "ClusterDirectory",
                    "validated portal front cell has no cluster membership",
                )
            })?;
        let back = cell_to_cluster
            .get(portal.back_leaf as usize)
            .and_then(|cluster| *cluster)
            .ok_or_else(|| {
                section_validation(
                    "ClusterDirectory",
                    "validated portal back cell has no cluster membership",
                )
            })?;
        if front != back {
            adjacency[front as usize].insert(back);
            adjacency[back as usize].insert(front);
        }
        let portal_id = u32::try_from(portal_id)
            .map_err(|_| section_validation("ClusterDirectory", "portal id exceeds u32"))?;
        if directory.seam_portal_ids.binary_search(&portal_id).is_ok() {
            if front == back {
                return Err(section_validation(
                    "ClusterDirectory",
                    "validated seam portal endpoints share a cluster",
                ));
            }
            seam_portals.push(ShStreamSeamPortal {
                portal_id,
                front_cluster_id: front,
                back_cluster_id: back,
            });
        }
    }
    if seam_portals.len() != directory.seam_portal_ids.len() {
        return Err(section_validation(
            "ClusterDirectory",
            "validated seam portal id is absent from portal topology",
        ));
    }
    Ok((
        adjacency
            .into_iter()
            .map(|neighbors| neighbors.into_iter().collect())
            .collect(),
        seam_portals,
    ))
}

fn base_projection(base: &crate::sh_stream::ShStreamBaseMetadata) -> OctahedralShVolumeSection {
    OctahedralShVolumeSection {
        grid_origin: base.grid_origin,
        cell_size: base.cell_size,
        grid_dimensions: base.grid_dimensions,
        probe_stride: base.probe_stride,
        tile_dimension: base.tile_dimension,
        tile_border: base.tile_border,
        atlas_dimensions: base.atlas_dimensions,
        layer_count: base.layer_count,
        tiles_per_layer: base.tiles_per_layer,
        atlas_tiles_per_row: base.atlas_tiles_per_row,
        probes: base.probes.clone(),
        irradiance_format: base.irradiance_format,
        // Directory semantic validation consumes only metadata; never create
        // a stand-in for the streamed compact atlas.
        compact_atlas: Vec::new(),
        animation_descriptors: base.animation_descriptors.clone(),
        slot_for_map_light: base.slot_for_map_light.clone(),
    }
}

fn direct_projection(direct: &crate::sh_stream::ShStreamDirectMetadata) -> DirectShVolumeSection {
    DirectShVolumeSection {
        grid_origin: direct.grid_origin,
        cell_size: direct.cell_size,
        grid_dimensions: direct.grid_dimensions,
        tile_dimension: direct.tile_dimension,
        tile_border: direct.tile_border,
        atlas_dimensions: direct.atlas_dimensions,
        layer_count: direct.layer_count,
        tiles_per_layer: direct.tiles_per_layer,
        atlas_tiles_per_row: direct.atlas_tiles_per_row,
        irradiance_format: direct.irradiance_format,
        atlas: Vec::new(),
    }
}

fn delta_projection(metadata: &crate::sh_stream::ShStreamSparseMetadata) -> DeltaShVolumesSection {
    DeltaShVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: metadata.affinity_dims,
        tile_dimension: metadata.tile_dimension,
        tile_border: metadata.tile_border,
        animation_descriptor_indices: metadata.animation_descriptor_indices.clone(),
        valid_probe_masks: metadata.valid_probe_masks.clone(),
        cell_levels: metadata.cell_levels.clone(),
        affinity_offsets: metadata.affinity_offsets.clone(),
        affinity_lights: metadata.affinity_lights.clone(),
        delta_subblocks: Vec::new(),
    }
}

fn direct_delta_projection(
    metadata: &crate::sh_stream::ShStreamSparseMetadata,
) -> DirectShDeltaVolumesSection {
    DirectShDeltaVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: metadata.affinity_dims,
        tile_dimension: metadata.tile_dimension,
        tile_border: metadata.tile_border,
        valid_probe_masks: metadata.valid_probe_masks.clone(),
        cell_levels: metadata.cell_levels.clone(),
        affinity_offsets: metadata.affinity_offsets.clone(),
        affinity_lights: metadata.affinity_lights.clone(),
        delta_subblocks: Vec::new(),
    }
}

fn animated_direct_delta_projection(
    metadata: &crate::sh_stream::ShStreamSparseMetadata,
) -> AnimatedDirectShDeltaVolumesSection {
    AnimatedDirectShDeltaVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: metadata.affinity_dims,
        tile_dimension: metadata.tile_dimension,
        tile_border: metadata.tile_border,
        animation_descriptor_indices: metadata.animation_descriptor_indices.clone(),
        valid_probe_masks: metadata.valid_probe_masks.clone(),
        cell_levels: metadata.cell_levels.clone(),
        affinity_offsets: metadata.affinity_offsets.clone(),
        affinity_lights: metadata.affinity_lights.clone(),
        delta_subblocks: Vec::new(),
    }
}

struct StreamingDirectoryProjections {
    base: OctahedralShVolumeSection,
    direct: Option<DirectShVolumeSection>,
    delta: Option<DeltaShVolumesSection>,
    direct_delta: Option<DirectShDeltaVolumesSection>,
    animated_direct_delta: Option<AnimatedDirectShDeltaVolumesSection>,
}

fn directory_projections(manifest: &ShStreamManifest) -> StreamingDirectoryProjections {
    let sources = manifest.sources();
    StreamingDirectoryProjections {
        base: base_projection(manifest.base()),
        direct: sources.direct.as_ref().map(direct_projection),
        delta: sources.indirect_delta.as_ref().map(delta_projection),
        direct_delta: sources.direct_delta.as_ref().map(direct_delta_projection),
        animated_direct_delta: sources
            .animated_direct_delta
            .as_ref()
            .map(animated_direct_delta_projection),
    }
}

/// Section 49 is optional, but when present its structural contract is
/// checked before any optional lighting fallback can affect diagnostics.
pub(super) fn read_cluster_directory_structure(
    container: &PrlContainer,
    streaming: Option<&ShStreamManifest>,
) -> Result<Option<ClusterDirectorySection>, PrlLoadError> {
    let meta = container.metadata();
    let read_section = |section: SectionId| container.read_section(section as u32);

    let directory_entries: Vec<_> = meta
        .sections
        .iter()
        .filter(|entry| entry.section_id == SectionId::ClusterDirectory as u32)
        .collect();
    if directory_entries.len() > 1 {
        return Err(ClusterDirectoryError::InvalidData(format!(
            "duplicate section 49 entries ({})",
            directory_entries.len()
        ))
        .into());
    }
    let parsed_cluster_directory = match directory_entries.first() {
        Some(entry) => {
            if entry.version != CLUSTER_DIRECTORY_CONTAINER_VERSION {
                return Err(ClusterDirectoryError::VersionMismatch {
                    version: u32::from(entry.version),
                    expected: u32::from(CLUSTER_DIRECTORY_CONTAINER_VERSION),
                }
                .into());
            }
            // A streaming manifest was built from this same file and table, so
            // it already holds this id 49 parsed and structurally validated;
            // the streaming branch below validates semantics on that copy.
            if streaming.is_some() {
                None
            } else {
                let data = read_section(SectionId::ClusterDirectory)?.ok_or_else(|| {
                    ClusterDirectoryError::InvalidData(
                        "section 49 table entry could not be read".into(),
                    )
                })?;
                Some(ClusterDirectorySection::from_bytes(&data)?)
            }
        }
        None => None,
    };
    Ok(parsed_cluster_directory)
}

/// Semantic id-49 validation against the decoded sections. In streaming
/// mode this also installs the cluster portal topology on the manifest.
pub(super) fn validate_cluster_directory(
    container: &PrlContainer,
    streaming: Option<&ShStreamManifest>,
    parsed_cluster_directory: Option<ClusterDirectorySection>,
    spatial: &SpatialValidationInputs,
    sh_volume: &Option<OctahedralShVolumeSection>,
    sh: &ShCompanions,
    mover_channel: Option<&MoverProbeChannelSection>,
) -> Result<Option<ClusterDirectorySection>, PrlLoadError> {
    // Id 57 is an id-49 resource only while a payload streams.
    let mover_channel = mover_channel.filter(|section| section.streams());
    let SpatialValidationInputs {
        bvh_section,
        portals_section,
        cells_section,
        cell_locator_section,
        ..
    } = spatial;
    let ShCompanions {
        delta_sh_volumes,
        parsed_animated_direct_sh_delta_volumes,
        parsed_billboard_direct_scatter_volume,
        parsed_animated_billboard_direct_scatter_delta_volumes,
        direct_sh_volume,
        parsed_entity_shadow_lights,
        direct_sh_delta_volumes,
        ..
    } = sh;
    let scatter_pair_is_usable = sh.scatter_pair_is_usable;

    let on_wire = |section: SectionId| container.has_section(section as u32);
    let streaming_directory_projections = streaming.map(directory_projections);
    let mut unavailable_directory_companions = Vec::new();
    for (section, available) in [
        (
            SectionId::DeltaShVolumes,
            streaming_directory_projections
                .as_ref()
                .map_or(delta_sh_volumes.is_some(), |projection| {
                    projection.delta.is_some()
                }),
        ),
        (
            SectionId::DirectShVolume,
            streaming_directory_projections
                .as_ref()
                .map_or(direct_sh_volume.is_some(), |projection| {
                    projection.direct.is_some()
                }),
        ),
        (
            SectionId::DirectShDeltaVolumes,
            streaming_directory_projections
                .as_ref()
                .map_or(direct_sh_delta_volumes.is_some(), |projection| {
                    projection.direct_delta.is_some()
                }),
        ),
        (
            SectionId::AnimatedDirectShDeltaVolumes,
            streaming_directory_projections.as_ref().map_or(
                parsed_animated_direct_sh_delta_volumes.is_some(),
                |projection| projection.animated_direct_delta.is_some(),
            ),
        ),
        (
            SectionId::BillboardDirectScatterVolume,
            parsed_billboard_direct_scatter_volume.is_some() && scatter_pair_is_usable,
        ),
        (
            SectionId::AnimatedBillboardDirectScatterDeltaVolumes,
            parsed_animated_billboard_direct_scatter_delta_volumes.is_some()
                && scatter_pair_is_usable,
        ),
    ] {
        if on_wire(section) && !available {
            unavailable_directory_companions.push(section as u32);
        }
    }

    let cluster_directory = if let Some(manifest) = streaming {
        if !unavailable_directory_companions.is_empty() {
            // A streaming session has to retain a complete, semantically
            // validated id-49 topology for the residency planner. Legacy maps
            // may soft-disable a malformed optional scatter companion, but
            // silently returning a manifest without its adjacency graph would
            // be unsafe; require the caller to use legacy/off mode instead.
            return Err(section_validation(
                "SH streaming",
                format!(
                    "cannot start streaming because ClusterDirectory companion section(s) {:?} are unavailable; retry with POSTRETRO_SH_STREAMING=off",
                    unavailable_directory_companions,
                ),
            ));
        }
        let projections = streaming_directory_projections
            .as_ref()
            .expect("streaming manifest always has directory projections");
        let empty_portals = PortalsSection {
            vertices: Vec::new(),
            portals: Vec::new(),
        };
        let directory = manifest.cluster_directory();
        directory.validate_semantics(ClusterDirectoryValidationInputs {
            cells: cells_section,
            portals: portals_section.as_ref().unwrap_or(&empty_portals),
            bvh: bvh_section,
            cell_locator: cell_locator_section,
            sh: ClusterDirectoryShInventory {
                octahedral: Some(&projections.base),
                direct: projections.direct.as_ref(),
                delta: projections.delta.as_ref(),
                shadow_selection: parsed_entity_shadow_lights.as_ref(),
                direct_delta: projections.direct_delta.as_ref(),
                animated_direct_delta: projections.animated_direct_delta.as_ref(),
                billboard: parsed_billboard_direct_scatter_volume.as_ref(),
                animated_billboard_delta: parsed_animated_billboard_direct_scatter_delta_volumes
                    .as_ref(),
                mover_channel,
            },
        })?;
        let (adjacency, seam_portals) = cluster_portal_topology_from_portals(
            directory,
            portals_section.as_ref().unwrap_or(&empty_portals),
        )?;
        manifest.install_cluster_portal_topology(adjacency, seam_portals)?;
        Some(directory.clone())
    } else {
        match parsed_cluster_directory {
            None => None,
            Some(_directory) if !unavailable_directory_companions.is_empty() => {
                log::warn!(
                    "[PRL] ClusterDirectoryUnavailableCompanion: section(s) {:?}; validated structural metadata remains inert and unavailable",
                    unavailable_directory_companions,
                );
                None
            }
            Some(directory) => {
                let empty_portals = PortalsSection {
                    vertices: Vec::new(),
                    portals: Vec::new(),
                };
                directory.validate_semantics(ClusterDirectoryValidationInputs {
                    cells: cells_section,
                    portals: portals_section.as_ref().unwrap_or(&empty_portals),
                    bvh: bvh_section,
                    cell_locator: cell_locator_section,
                    sh: ClusterDirectoryShInventory {
                        octahedral: sh_volume.as_ref(),
                        direct: direct_sh_volume.as_ref(),
                        delta: delta_sh_volumes.as_ref(),
                        shadow_selection: parsed_entity_shadow_lights.as_ref(),
                        direct_delta: direct_sh_delta_volumes.as_ref(),
                        animated_direct_delta: parsed_animated_direct_sh_delta_volumes.as_ref(),
                        billboard: parsed_billboard_direct_scatter_volume.as_ref(),
                        animated_billboard_delta:
                            parsed_animated_billboard_direct_scatter_delta_volumes.as_ref(),
                        mover_channel,
                    },
                })?;
                Some(directory)
            }
        }
    };

    Ok(cluster_directory)
}
