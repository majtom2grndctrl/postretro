//! Compiler-side id-50 chunk publication.
//!
//! The format crate owns the persistent wire contract and validates the result.
//! This module only gathers final compiler products into that contract.  In
//! particular, it keeps the legacy id-34/id-35 bodies out of the calculation:
//! isolated cells are built from the borrowed pre-BC6H sources retained by
//! [`super::FinalizedShPackSources`].

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

use postretro_level_format::SectionId;
use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_format::cluster_sh_payloads::{
    CLUSTER_SH_PAYLOADS_CONTAINER_VERSION, ClusterShPayloadsBaseMetadata, ClusterShPayloadsHeader,
    ClusterShPayloadsIndexRecord, ClusterShPayloadsSection, ClusterShPayloadsSourceRecord,
    ClusterShPayloadsValidationInputs, source_metadata_from_sections,
};
use tempfile::{Builder as TempFileBuilder, NamedTempFile};

use super::{FinalizedShEmissionView, FinalizedShPackSources, PlannedSection};

#[path = "cluster_sh_payloads/atlas.rs"]
mod atlas;
#[path = "cluster_sh_payloads/dense.rs"]
mod dense;
#[path = "cluster_sh_payloads/sparse.rs"]
mod sparse;
#[path = "cluster_sh_payloads/wire.rs"]
mod wire;

use dense::ChunkPlan;
use dense::DenseNode;
use sparse::SparseSources;
use wire::{
    BLOCK_KIND_ISOLATED_ATLAS, BLOCK_KIND_PROBE_PATCHES, EncodedBlock, EncodedSparse,
    encode_chunk_wire, push_u32,
};

/// An id-50 prefix and its payload spool. Dropping this value removes the
/// spool on every path; its writer moves the exact bytes directly to the
/// staged PRL without materializing a full payload vector.
pub(super) struct ClusterPayloadSpool {
    section: ClusterShPayloadsSection,
    payload_len: u64,
    spool: NamedTempFile,
}

impl ClusterPayloadSpool {
    pub(super) fn into_planned_section(self) -> anyhow::Result<PlannedSection<'static>> {
        let metadata = self
            .section
            .metadata_bytes()
            .map_err(|error| anyhow::anyhow!("id-50 metadata encode failed: {error}"))?;
        let expected_len = u64::try_from(metadata.len())
            .map_err(|_| anyhow::anyhow!("id-50 metadata length exceeds u64"))?
            .checked_add(self.payload_len)
            .ok_or_else(|| anyhow::anyhow!("id-50 section length overflow"))?;
        Ok(PlannedSection::with_writer(
            SectionId::ClusterShPayloads as u32,
            CLUSTER_SH_PAYLOADS_CONTAINER_VERSION,
            expected_len,
            move |writer| {
                writer.write_all(&metadata)?;
                let mut spool = self.spool.reopen()?;
                std::io::copy(&mut spool, writer)?;
                Ok(())
            },
        ))
    }
}

pub(super) fn build_cluster_payload_spool(
    output: &Path,
    directory: &ClusterDirectorySection,
    emission: FinalizedShEmissionView<'_>,
    sources: FinalizedShPackSources<'_>,
) -> anyhow::Result<ClusterPayloadSpool> {
    let metadata_sources = source_metadata_from_sections(
        emission.octahedral,
        emission.direct,
        emission.delta,
        emission.direct_delta,
        emission.animated_direct_delta,
    );
    let validation_inputs = ClusterShPayloadsValidationInputs {
        directory,
        base: ClusterShPayloadsBaseMetadata::from(emission.octahedral),
        sources: &metadata_sources,
    };
    let sparse_sources = SparseSources::from_sections(
        emission.delta,
        emission.direct_delta,
        emission.animated_direct_delta,
    )?;
    let plan = ChunkPlan::new(validation_inputs, sparse_sources)?;
    let mut spool = create_spool(output)?;
    let mut index = Vec::with_capacity(directory.clusters.len());
    let mut payload_len = 0u64;

    for cluster_id in 0..u32::try_from(directory.clusters.len())? {
        let chunk = plan.encode_chunk(cluster_id, sources.octahedral, sources.direct)?;
        let chunk_len = u64::try_from(chunk.bytes.len())?;
        spool.as_file_mut().write_all(&chunk.bytes)?;
        index.push(ClusterShPayloadsIndexRecord {
            payload_offset: payload_len,
            payload_len: chunk_len,
            decoded_bytes: chunk.decoded_bytes,
            requested_resident_bytes: chunk.requested_resident_bytes,
            stored_tile_count: chunk.stored_tile_count,
            dense_patch_count: chunk.dense_patch_count,
            affinity_patch_count: chunk.affinity_patch_count,
            hash: *blake3::hash(&chunk.bytes).as_bytes(),
        });
        payload_len = payload_len
            .checked_add(chunk_len)
            .ok_or_else(|| anyhow::anyhow!("id-50 payload byte count overflow"))?;
    }
    spool.as_file_mut().flush()?;

    let section = ClusterShPayloadsSection {
        header: ClusterShPayloadsHeader {
            cluster_count: u32::try_from(directory.clusters.len())?,
            source_count: u32::try_from(metadata_sources.len())?,
            grid_dimensions: emission.octahedral.grid_dimensions,
            affinity_dimensions: plan.affinity_dimensions,
            payload_bytes: payload_len,
        },
        sources: metadata_sources
            .iter()
            .map(|source| ClusterShPayloadsSourceRecord {
                section_id: source.section_id(),
                internal_version: source.internal_version(),
                kind: source.kind(),
            })
            .collect(),
        index,
    };
    section
        .validate_against(validation_inputs)
        .map_err(|error| {
            anyhow::anyhow!("id-50 metadata disagrees with final SH sources: {error}")
        })?;

    Ok(ClusterPayloadSpool {
        section,
        payload_len,
        spool,
    })
}

fn create_spool(output: &Path) -> anyhow::Result<NamedTempFile> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = output
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("output path has no file name: {}", output.display()))?;
    let mut prefix = OsString::from(".");
    prefix.push(file_name);
    prefix.push(".cluster-sh-");
    TempFileBuilder::new()
        .prefix(&prefix)
        .suffix(".spool")
        .tempfile_in(parent)
        .map_err(|error| {
            anyhow::anyhow!(
                "failed to create session-owned cluster SH spool beside {}: {error}",
                output.display()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::dense::{
        PROBE_INDIRECTION_LEVEL_MASK, PROBE_INDIRECTION_SCALE_SHIFT, PROBE_INDIRECTION_SLOT_SHIFT,
    };
    use super::wire::BLOCK_KIND_SPARSE_ROWS;
    use super::*;
    use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection;
    use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
    use postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection;
    use postretro_level_format::bvh::{BvhLeaf, BvhSection};
    use postretro_level_format::cells::{CELL_FLAG_DRAWABLE, CellRecord, CellsSection};
    use postretro_level_format::cluster_directory::{
        CLUSTER_DIRECTORY_CONTAINER_VERSION, ClusterRangeRecord, ClusterRangeRole, ClusterRecord,
        ClusterResourceDomain, ClusterResourceRecord, DENSE_OWNER_SENTINEL,
        canonical_cell_partition,
    };
    use postretro_level_format::cluster_sh_payloads::{
        CLUSTER_SH_LOGICAL_TILE_DIMENSION, CLUSTER_SH_PAYLOADS_CONTAINER_VERSION,
        ClusterShPayloadsBaseMetadata, ClusterShPayloadsValidationInputs,
        source_metadata_from_sections,
    };
    use postretro_level_format::delta_sh_volumes::{
        AFFINITY_FACTOR, DEFAULT_DELTA_PROBE_F16_STRIDE, DeltaShVolumesSection,
    };
    use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
    use postretro_level_format::direct_sh_volume::{
        DIRECT_SH_VOLUME_VERSION, DirectShVolumeSection,
    };
    use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
    use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
    use postretro_level_format::octahedral::{
        MAX_SH_ATLAS_DIMENSION, irradiance_array_tile_location, irradiance_atlas_array_layout,
    };
    use postretro_level_format::portals::{PortalRecord, PortalsSection};
    use postretro_level_format::sh_reconstruct::Level;
    use postretro_level_format::sh_volume::{
        OCTAHEDRAL_PROBE_STRIDE, OctahedralShProbe, OctahedralShVolumeSection, SH_VOLUME_VERSION,
        validate_probe_metadata,
    };

    fn raw_octahedral_source() -> OctahedralShVolumeSection {
        let layout = irradiance_atlas_array_layout(
            [64, 1, 1],
            CLUSTER_SH_LOGICAL_TILE_DIMENSION,
            MAX_SH_ATLAS_DIMENSION,
        )
        .expect("64 isolated source tiles must fit the legacy atlas cap");
        let byte_len = layout.layer_count as usize
            * layout.atlas_width as usize
            * layout.atlas_height as usize
            * 8;
        let mut compact_atlas = vec![0; byte_len];
        for (index, texel) in compact_atlas.chunks_exact_mut(8).enumerate() {
            let value = u16::try_from(index).unwrap_or(u16::MAX).to_le_bytes();
            for channel in texel.chunks_exact_mut(2) {
                channel.copy_from_slice(&value);
            }
        }
        OctahedralShVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: [4, 4, 4],
            probe_stride: OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: CLUSTER_SH_LOGICAL_TILE_DIMENSION,
            tile_border: 1,
            atlas_dimensions: [layout.atlas_width, layout.atlas_height],
            layer_count: layout.layer_count,
            tiles_per_layer: layout.tiles_per_layer,
            atlas_tiles_per_row: layout.atlas_tiles_per_row,
            probes: vec![
                OctahedralShProbe {
                    validity: 1,
                    mean_distance: 0x3c00,
                    mean_sq_distance: 0x4000,
                    density_level: Level::L0.to_u8(),
                    node_scale: 0,
                };
                64
            ],
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            compact_atlas,
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    fn raw_direct_source(base: &OctahedralShVolumeSection) -> DirectShVolumeSection {
        DirectShVolumeSection {
            grid_origin: base.grid_origin,
            cell_size: base.cell_size,
            grid_dimensions: base.grid_dimensions,
            tile_dimension: base.tile_dimension,
            tile_border: base.tile_border,
            atlas_dimensions: base.atlas_dimensions,
            layer_count: base.layer_count,
            tiles_per_layer: base.tiles_per_layer,
            atlas_tiles_per_row: base.atlas_tiles_per_row,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            atlas: base.compact_atlas.clone(),
        }
    }

    fn raw_billboard_scatter_source() -> BillboardDirectScatterVolumeSection {
        BillboardDirectScatterVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: [4, 4, 4],
            scatter_rgba: vec![0x3c00; 64 * 4],
        }
    }

    fn raw_animated_billboard_scatter_source() -> AnimatedBillboardDirectScatterDeltaVolumesSection
    {
        AnimatedBillboardDirectScatterDeltaVolumesSection {
            animation_descriptor_indices: Vec::new(),
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            affinity_offsets: vec![0, 0],
            affinity_lights: Vec::new(),
            delta_rgba: Vec::new(),
        }
    }

    fn planned_legacy_section(
        section_id: SectionId,
        version: u16,
        body: Vec<u8>,
    ) -> PlannedSection<'static> {
        let byte_len = body.len();
        PlannedSection::new(section_id as u32, version, byte_len, move || Ok(body))
    }

    fn raw_scaled_l1_source() -> OctahedralShVolumeSection {
        let grid_dimensions = [8, 8, 8];
        let probes = vec![
            OctahedralShProbe {
                validity: 1,
                mean_distance: 0x3c00,
                mean_sq_distance: 0x4000,
                density_level: Level::L1.to_u8(),
                node_scale: 1,
            };
            8 * 8 * 8
        ];
        let prefix = validate_probe_metadata(grid_dimensions, &probes).unwrap();
        assert_eq!(prefix.total_stored_tiles, 8);
        let layout = irradiance_atlas_array_layout(
            [prefix.total_stored_tiles, 1, 1],
            CLUSTER_SH_LOGICAL_TILE_DIMENSION,
            MAX_SH_ATLAS_DIMENSION,
        )
        .unwrap();
        let mut compact_atlas = vec![
            0;
            layout.layer_count as usize
                * layout.atlas_width as usize
                * layout.atlas_height as usize
                * 8
        ];
        for slot in 0..prefix.total_stored_tiles {
            let [layer, tile_x, tile_y] = irradiance_array_tile_location(
                slot as usize,
                layout.tiles_per_layer,
                layout.atlas_tiles_per_row,
            );
            for y in 0..CLUSTER_SH_LOGICAL_TILE_DIMENSION as usize {
                for x in 0..CLUSTER_SH_LOGICAL_TILE_DIMENSION as usize {
                    let offset = ((layer as usize
                        * layout.atlas_width as usize
                        * layout.atlas_height as usize)
                        + (tile_y as usize * CLUSTER_SH_LOGICAL_TILE_DIMENSION as usize + y)
                            * layout.atlas_width as usize
                        + tile_x as usize * CLUSTER_SH_LOGICAL_TILE_DIMENSION as usize
                        + x)
                        * 8;
                    let value = u16::try_from(slot + 1).unwrap().to_le_bytes();
                    for channel in compact_atlas[offset..offset + 8].chunks_exact_mut(2) {
                        channel.copy_from_slice(&value);
                    }
                }
            }
        }
        OctahedralShVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions,
            probe_stride: OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: CLUSTER_SH_LOGICAL_TILE_DIMENSION,
            tile_border: 1,
            atlas_dimensions: [layout.atlas_width, layout.atlas_height],
            layer_count: layout.layer_count,
            tiles_per_layer: layout.tiles_per_layer,
            atlas_tiles_per_row: layout.atlas_tiles_per_row,
            probes,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            compact_atlas,
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    fn raw_delta_source() -> DeltaShVolumesSection {
        raw_delta_source_for_light(0, 0x3c00)
    }

    fn raw_delta_source_for_light(light: u32, value: u16) -> DeltaShVolumesSection {
        DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: CLUSTER_SH_LOGICAL_TILE_DIMENSION,
            tile_border: 1,
            animation_descriptor_indices: vec![0; light as usize + 1],
            valid_probe_masks: vec![u64::MAX],
            cell_levels: vec![Level::L0.to_u8()],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![light],
            delta_subblocks: vec![value; 64 * DEFAULT_DELTA_PROBE_F16_STRIDE],
        }
    }

    fn raw_direct_delta_source(light: u32, value: u16) -> DirectShDeltaVolumesSection {
        DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: CLUSTER_SH_LOGICAL_TILE_DIMENSION,
            tile_border: 1,
            valid_probe_masks: vec![u64::MAX],
            cell_levels: vec![Level::L0.to_u8()],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![light],
            delta_subblocks: vec![value; 64 * DEFAULT_DELTA_PROBE_F16_STRIDE],
        }
    }

    fn raw_animated_direct_delta_source(
        light: u32,
        value: u16,
    ) -> AnimatedDirectShDeltaVolumesSection {
        AnimatedDirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: CLUSTER_SH_LOGICAL_TILE_DIMENSION,
            tile_border: 1,
            animation_descriptor_indices: vec![0; light as usize + 1],
            valid_probe_masks: vec![u64::MAX],
            cell_levels: vec![Level::L0.to_u8()],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![light],
            delta_subblocks: vec![value; 64 * DEFAULT_DELTA_PROBE_F16_STRIDE],
        }
    }

    fn directory(with_direct: bool, with_delta: bool) -> ClusterDirectorySection {
        let mut resources = Vec::new();
        if with_delta {
            resources.push(ClusterResourceRecord {
                section_id: SectionId::DeltaShVolumes as u32,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1, 1, 1],
            });
        }
        resources.push(ClusterResourceRecord {
            section_id: SectionId::OctahedralShVolume as u32,
            domain: ClusterResourceDomain::DenseProbe,
            dimensions: [4, 4, 4],
        });
        if with_direct {
            resources.push(ClusterResourceRecord {
                section_id: SectionId::DirectShVolume as u32,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: [4, 4, 4],
            });
        }
        let base_resource = u32::from(with_delta);
        let mut ranges = Vec::new();
        if with_delta {
            ranges.push(ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 1,
                owner_cluster_id: 0,
                role: ClusterRangeRole::Owned,
            });
        }
        ranges.push(ClusterRangeRecord {
            resource_index: base_resource,
            start: 0,
            count: 64,
            owner_cluster_id: DENSE_OWNER_SENTINEL,
            role: ClusterRangeRole::Dense,
        });
        if with_direct {
            ranges.push(ClusterRangeRecord {
                resource_index: base_resource + 1,
                start: 0,
                count: 64,
                owner_cluster_id: DENSE_OWNER_SENTINEL,
                role: ClusterRangeRole::Dense,
            });
        }
        ClusterDirectorySection {
            runtime_cell_count: 1,
            primitive_limit: 64,
            cell_limit: 32,
            clusters: vec![ClusterRecord {
                bounds_min: [0.0; 3],
                bounds_max: [1.0; 3],
                member_start: 0,
                member_count: 1,
                range_start: 0,
                range_count: ranges.len() as u32,
                primitive_count: 0,
                flags: 0,
            }],
            resources,
            members: vec![0],
            ranges,
            seam_portal_ids: Vec::new(),
            cluster_hints: Vec::new(),
        }
    }

    fn directory_with_owned_and_halo_sparse_rows() -> ClusterDirectorySection {
        let resources = vec![
            ClusterResourceRecord {
                section_id: SectionId::DeltaShVolumes as u32,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1, 1, 1],
            },
            ClusterResourceRecord {
                section_id: SectionId::OctahedralShVolume as u32,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: [4, 4, 4],
            },
            ClusterResourceRecord {
                section_id: SectionId::DirectShVolume as u32,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: [4, 4, 4],
            },
            ClusterResourceRecord {
                section_id: SectionId::DirectShDeltaVolumes as u32,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1, 1, 1],
            },
            ClusterResourceRecord {
                section_id: SectionId::AnimatedDirectShDeltaVolumes as u32,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1, 1, 1],
            },
        ];
        let ranges_for = |role| {
            vec![
                ClusterRangeRecord {
                    resource_index: 0,
                    start: 0,
                    count: 1,
                    owner_cluster_id: 0,
                    role,
                },
                ClusterRangeRecord {
                    resource_index: 1,
                    start: 0,
                    count: 64,
                    owner_cluster_id: DENSE_OWNER_SENTINEL,
                    role: ClusterRangeRole::Dense,
                },
                ClusterRangeRecord {
                    resource_index: 2,
                    start: 0,
                    count: 64,
                    owner_cluster_id: DENSE_OWNER_SENTINEL,
                    role: ClusterRangeRole::Dense,
                },
                ClusterRangeRecord {
                    resource_index: 3,
                    start: 0,
                    count: 1,
                    owner_cluster_id: 0,
                    role,
                },
                ClusterRangeRecord {
                    resource_index: 4,
                    start: 0,
                    count: 1,
                    owner_cluster_id: 0,
                    role,
                },
            ]
        };
        let mut ranges = ranges_for(ClusterRangeRole::Owned);
        ranges.extend(ranges_for(ClusterRangeRole::Halo));
        ClusterDirectorySection {
            runtime_cell_count: 2,
            primitive_limit: 64,
            cell_limit: 32,
            clusters: vec![
                ClusterRecord {
                    bounds_min: [0.0; 3],
                    bounds_max: [1.0; 3],
                    member_start: 0,
                    member_count: 1,
                    range_start: 0,
                    range_count: 5,
                    primitive_count: 0,
                    flags: 0,
                },
                ClusterRecord {
                    bounds_min: [1.0, 0.0, 0.0],
                    bounds_max: [2.0, 1.0, 1.0],
                    member_start: 1,
                    member_count: 1,
                    range_start: 5,
                    range_count: 5,
                    primitive_count: 0,
                    flags: 0,
                },
            ],
            resources,
            members: vec![0, 1],
            ranges,
            seam_portal_ids: Vec::new(),
            cluster_hints: Vec::new(),
        }
    }

    fn scaled_l1_directory() -> ClusterDirectorySection {
        ClusterDirectorySection {
            runtime_cell_count: 1,
            primitive_limit: 64,
            cell_limit: 32,
            clusters: vec![ClusterRecord {
                bounds_min: [0.0; 3],
                bounds_max: [1.0; 3],
                member_start: 0,
                member_count: 1,
                range_start: 0,
                range_count: 1,
                primitive_count: 0,
                flags: 0,
            }],
            resources: vec![ClusterResourceRecord {
                section_id: SectionId::OctahedralShVolume as u32,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: [8, 8, 8],
            }],
            members: vec![0],
            ranges: vec![ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 8 * 8 * 8,
                owner_cluster_id: DENSE_OWNER_SENTINEL,
                role: ClusterRangeRole::Dense,
            }],
            seam_portal_ids: Vec::new(),
            cluster_hints: Vec::new(),
        }
    }

    fn encoded_spool_bytes(
        output: &Path,
        directory: &ClusterDirectorySection,
        emission: FinalizedShEmissionView<'_>,
        sources: FinalizedShPackSources<'_>,
    ) -> Vec<u8> {
        let spool = build_cluster_payload_spool(output, directory, emission, sources)
            .expect("fixture must encode id 50");
        let mut bytes = spool.section.metadata_bytes().unwrap();
        bytes.extend_from_slice(&std::fs::read(spool.spool.path()).unwrap());
        bytes
    }

    // Slice 4 baseline: this fixed, no-hint compiler fixture freezes the
    // independently readable id-50 bytes before id-49 partition/wire changes.
    #[test]
    fn no_hint_two_cell_fixture_preserves_id50_hash_and_cell_membership() {
        let cells = CellsSection {
            cells: vec![
                CellRecord {
                    bounds_min: [0.0; 3],
                    bounds_max: [1.0; 3],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 0,
                    portal_ref_count: 0,
                },
                CellRecord {
                    bounds_min: [1.0, 0.0, 0.0],
                    bounds_max: [2.0, 1.0, 1.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 0,
                    portal_ref_count: 0,
                },
            ],
            portal_refs: Vec::new(),
        };
        let portals = PortalsSection {
            vertices: vec![
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 1.0, 1.0],
                [1.0, 0.0, 1.0],
            ],
            portals: vec![PortalRecord {
                vertex_start: 0,
                vertex_count: 4,
                front_leaf: 0,
                back_leaf: 1,
            }],
        };
        let bvh = BvhSection {
            nodes: Vec::new(),
            leaves: [0, 1]
                .into_iter()
                .map(|cell_id| BvhLeaf {
                    aabb_min: [cell_id as f32, 0.0, 0.0],
                    material_bucket_id: 0,
                    aabb_max: [cell_id as f32 + 1.0, 1.0, 1.0],
                    index_offset: 0,
                    index_count: 3,
                    cell_id,
                    chunk_range_start: 0,
                    chunk_range_count: 0,
                })
                .collect(),
            root_node_index: 0,
        };
        let partition = canonical_cell_partition(&cells, &portals, &bvh, 64, 32, &[]).unwrap();
        assert_eq!(partition.members, vec![0, 1]);
        assert_eq!(
            partition
                .clusters
                .iter()
                .map(|cluster| (cluster.member_start, cluster.member_count))
                .collect::<Vec<_>>(),
            vec![(0, 2)]
        );

        let base = raw_octahedral_source();
        let direct = raw_direct_source(&base);
        let delta = raw_delta_source_for_light(7, 0x3c00);
        let direct_delta = raw_direct_delta_source(0, 0x3555);
        let animated_delta = raw_animated_direct_delta_source(11, 0x3666);
        let selection = EntityShadowLightsSection {
            light_indices: vec![99],
        };
        let directory = directory_with_owned_and_halo_sparse_rows();
        let emission = FinalizedShEmissionView::new(
            &base,
            Some(&direct),
            Some(&delta),
            Some(&selection),
            Some(&direct_delta),
            Some(&animated_delta),
            None,
            None,
        )
        .unwrap();
        let sources = FinalizedShPackSources::new(&base, Some(&direct)).unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let bytes = encoded_spool_bytes(
            &tempdir.path().join("no-hint-fixture.prl"),
            &directory,
            emission,
            sources,
        );

        assert_eq!(directory.members, vec![0, 1]);
        assert_eq!(
            blake3::hash(&bytes).to_hex().as_str(),
            "817ab1de29cad06ddb23245954f7617fe8f275ae96be4e4237c480800045d960"
        );
    }

    #[test]
    fn cluster_payload_spool_is_deterministic_across_dedicated_rayon_pools() {
        let base = raw_octahedral_source();
        let directory = directory(false, false);
        let emission =
            FinalizedShEmissionView::new(&base, None, None, None, None, None, None, None).unwrap();
        let sources = FinalizedShPackSources::new(&base, None).unwrap();
        let first_dir = tempfile::tempdir().unwrap();
        let second_dir = tempfile::tempdir().unwrap();
        let first_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let second_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        let first = first_pool.install(|| {
            encoded_spool_bytes(
                &first_dir.path().join("one.prl"),
                &directory,
                emission,
                sources,
            )
        });
        let second = second_pool.install(|| {
            encoded_spool_bytes(
                &second_dir.path().join("four.prl"),
                &directory,
                emission,
                sources,
            )
        });
        assert_eq!(
            first, second,
            "id-50 bytes must not depend on Rayon worker count"
        );
    }

    #[test]
    fn cluster_payload_keeps_legacy_bodies_and_excludes_billboard_scatter() {
        let base = raw_octahedral_source();
        let direct = raw_direct_source(&base);
        let delta = raw_delta_source();
        let directory = directory(true, true);
        let base_before = base.try_to_bytes().unwrap();
        let direct_before = direct.try_to_bytes().unwrap();
        let emission = FinalizedShEmissionView::new(
            &base,
            Some(&direct),
            Some(&delta),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let sources = FinalizedShPackSources::new(&base, Some(&direct)).unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let spool = build_cluster_payload_spool(
            &tempdir.path().join("fixture.prl"),
            &directory,
            emission,
            sources,
        )
        .unwrap();
        assert_eq!(base.try_to_bytes().unwrap(), base_before);
        assert_eq!(direct.try_to_bytes().unwrap(), direct_before);
        assert_eq!(
            spool
                .section
                .sources
                .iter()
                .map(|source| source.section_id)
                .collect::<Vec<_>>(),
            vec![
                SectionId::DeltaShVolumes as u32,
                SectionId::OctahedralShVolume as u32,
                SectionId::DirectShVolume as u32,
            ],
        );
        assert!(spool.section.sources.iter().all(|source| {
            source.section_id != SectionId::BillboardDirectScatterVolume as u32
                && source.section_id != SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32
        }));
        assert_eq!(spool.section.index[0].affinity_patch_count, 1);
    }

    #[test]
    fn cluster_payload_keeps_owned_and_halo_rows_for_each_sparse_namespace() {
        let base = raw_octahedral_source();
        let direct = raw_direct_source(&base);
        let delta = raw_delta_source_for_light(7, 0x3c00);
        let direct_delta = raw_direct_delta_source(0, 0x3555);
        let animated_delta = raw_animated_direct_delta_source(11, 0x3666);
        let selection = EntityShadowLightsSection {
            light_indices: vec![99],
        };
        let directory = directory_with_owned_and_halo_sparse_rows();
        let emission = FinalizedShEmissionView::new(
            &base,
            Some(&direct),
            Some(&delta),
            Some(&selection),
            Some(&direct_delta),
            Some(&animated_delta),
            None,
            None,
        )
        .unwrap();
        let sources = FinalizedShPackSources::new(&base, Some(&direct)).unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let spool = build_cluster_payload_spool(
            &tempdir.path().join("fixture.prl"),
            &directory,
            emission,
            sources,
        )
        .unwrap();
        let metadata_sources = source_metadata_from_sections(
            &base,
            Some(&direct),
            Some(&delta),
            Some(&direct_delta),
            Some(&animated_delta),
        );
        let inputs = ClusterShPayloadsValidationInputs {
            directory: &directory,
            base: ClusterShPayloadsBaseMetadata::from(&base),
            sources: &metadata_sources,
        };
        let payload = std::fs::read(spool.spool.path()).unwrap();
        let owned_range = spool.section.payload_range(0).unwrap();
        let halo_range = spool.section.payload_range(1).unwrap();
        let decoded_owned = spool
            .section
            .decode_chunk(
                0,
                payload[usize::try_from(owned_range.start).unwrap()
                    ..usize::try_from(owned_range.end).unwrap()]
                    .to_vec(),
                inputs,
            )
            .unwrap();
        let decoded_halo = spool
            .section
            .decode_chunk(
                1,
                payload[usize::try_from(halo_range.start).unwrap()
                    ..usize::try_from(halo_range.end).unwrap()]
                    .to_vec(),
                inputs,
            )
            .unwrap();
        for (section_id, expected_light, expected_value) in [
            (SectionId::DeltaShVolumes as u32, 7, 0x3c00),
            (SectionId::DirectShDeltaVolumes as u32, 0, 0x3555),
            (SectionId::AnimatedDirectShDeltaVolumes as u32, 11, 0x3666),
        ] {
            let owned = decoded_owned
                .blocks
                .iter()
                .find(|block| {
                    block.section_id == section_id && block.kind == BLOCK_KIND_SPARSE_ROWS
                })
                .unwrap();
            let halo = decoded_halo
                .blocks
                .iter()
                .find(|block| {
                    block.section_id == section_id && block.kind == BLOCK_KIND_SPARSE_ROWS
                })
                .unwrap();
            let owned_body = &decoded_owned.bytes[owned.body.clone()];
            let halo_body = &decoded_halo.bytes[halo.body.clone()];
            assert_eq!(
                u32::from_le_bytes(owned_body[12..16].try_into().unwrap()),
                0
            );
            assert_eq!(
                u32::from_le_bytes(owned_body[28..32].try_into().unwrap()),
                ClusterRangeRole::Owned as u32
            );
            assert_eq!(
                u32::from_le_bytes(halo_body[28..32].try_into().unwrap()),
                ClusterRangeRole::Halo as u32
            );
            assert_eq!(
                u32::from_le_bytes(owned_body[32..36].try_into().unwrap()),
                expected_light
            );
            assert_eq!(
                u16::from_le_bytes(owned_body[48..50].try_into().unwrap()),
                expected_value
            );
            let mut halo_with_owned_role = halo_body.to_vec();
            halo_with_owned_role[28..32]
                .copy_from_slice(&(ClusterRangeRole::Owned as u32).to_le_bytes());
            assert_eq!(
                owned_body, halo_with_owned_role,
                "halo must retain the owned row payload"
            );
        }
        assert!(spool.section.index[0].requested_resident_bytes > 0);
        assert_eq!(spool.section.index[1].requested_resident_bytes, 0);
    }

    #[test]
    fn cluster_payload_preserves_scaled_l1_node_base_and_all_corner_cells() {
        let base = raw_scaled_l1_source();
        let directory = scaled_l1_directory();
        let emission =
            FinalizedShEmissionView::new(&base, None, None, None, None, None, None, None).unwrap();
        let sources = FinalizedShPackSources::new(&base, None).unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let spool = build_cluster_payload_spool(
            &tempdir.path().join("scaled-l1.prl"),
            &directory,
            emission,
            sources,
        )
        .unwrap();
        let metadata_sources = source_metadata_from_sections(&base, None, None, None, None);
        let inputs = ClusterShPayloadsValidationInputs {
            directory: &directory,
            base: ClusterShPayloadsBaseMetadata::from(&base),
            sources: &metadata_sources,
        };
        let decoded = spool
            .section
            .decode_chunk(0, std::fs::read(spool.spool.path()).unwrap(), inputs)
            .unwrap();
        assert_eq!(spool.section.index[0].stored_tile_count, 8);
        let patches = decoded
            .blocks
            .iter()
            .find(|block| block.kind == BLOCK_KIND_PROBE_PATCHES)
            .unwrap();
        let patch_body = &decoded.bytes[patches.body.clone()];
        for patch in patch_body.chunks_exact(16) {
            let word = u32::from_le_bytes(patch[4..8].try_into().unwrap());
            assert_eq!(word >> PROBE_INDIRECTION_SLOT_SHIFT, 0);
            assert_eq!(
                word & PROBE_INDIRECTION_LEVEL_MASK,
                Level::L1.to_u8() as u32
            );
            assert_eq!(word >> PROBE_INDIRECTION_SCALE_SHIFT & 0x3, 1);
        }
        let atlas = decoded
            .blocks
            .iter()
            .find(|block| {
                block.section_id == SectionId::OctahedralShVolume as u32
                    && block.kind == BLOCK_KIND_ISOLATED_ATLAS
            })
            .unwrap();
        let body = &decoded.bytes[atlas.body.clone()];
        let width = u32::from_le_bytes(body[8..12].try_into().unwrap());
        let height = u32::from_le_bytes(body[12..16].try_into().unwrap());
        let tiles_per_row = width / 8;
        let tiles_per_layer = tiles_per_row * (height / 8);
        let payload = &body[20..];
        for slot in 0..8u32 {
            let [layer, tile_x, tile_y] =
                irradiance_array_tile_location(slot as usize, tiles_per_layer, tiles_per_row);
            let offset = ((layer * width * height + tile_y * 8 * width + tile_x * 8) * 8) as usize;
            assert_eq!(
                u16::from_le_bytes(payload[offset..offset + 2].try_into().unwrap()),
                slot as u16 + 1,
                "scaled L1 corner slot {slot} must retain its canonical base-plus-corner source tile",
            );
        }
    }

    #[test]
    fn failed_cluster_payload_encoding_removes_its_spool() {
        let base = raw_octahedral_source();
        let mut malformed_source = base.clone();
        malformed_source.compact_atlas.clear();
        let directory = directory(false, false);
        let emission =
            FinalizedShEmissionView::new(&base, None, None, None, None, None, None, None).unwrap();
        let sources = FinalizedShPackSources::new(&malformed_source, None).unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let output = tempdir.path().join("fixture.prl");
        let before = std::fs::read_dir(tempdir.path()).unwrap().count();
        assert!(build_cluster_payload_spool(&output, &directory, emission, sources).is_err());
        let after = std::fs::read_dir(tempdir.path()).unwrap().count();
        assert_eq!(after, before, "failed id-50 emission must remove its spool");
    }

    #[test]
    fn failed_staged_writer_removes_cluster_payload_spool() {
        let base = raw_octahedral_source();
        let directory = directory(false, false);
        let emission =
            FinalizedShEmissionView::new(&base, None, None, None, None, None, None, None).unwrap();
        let sources = FinalizedShPackSources::new(&base, None).unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let output = tempdir.path().join("fixture.prl");
        let spool = build_cluster_payload_spool(&output, &directory, emission, sources).unwrap();
        let spool_path = spool.spool.path().to_path_buf();
        let error = super::super::write_and_validate_sections(
            &output,
            vec![
                spool.into_planned_section().unwrap(),
                PlannedSection::with_writer(SectionId::Geometry as u32, 1, 1, |_writer| {
                    anyhow::bail!("test staged writer failure")
                }),
            ],
        )
        .expect_err("a writer after id-50 must fail staging");
        assert!(error.to_string().contains("test staged writer failure"));
        assert!(
            !spool_path.exists(),
            "id-50 spool must drop after staged writer failure"
        );
        assert!(!output.exists(), "failed staging must not publish a PRL");
    }

    #[test]
    fn staged_cluster_payload_appends_after_every_legacy_body_unchanged() {
        let raw_base = raw_octahedral_source();
        let raw_direct = raw_direct_source(&raw_base);
        let base = crate::sh_bake::encode_sh_volume_section_bc6h(&raw_base, false);
        let direct = crate::direct_sh_bake::encode_direct_section_bc6h(&raw_direct, false);
        let billboard = raw_billboard_scatter_source();
        let animated_billboard = raw_animated_billboard_scatter_source();
        let directory = directory(true, false);
        let directory_body = directory.try_to_bytes().unwrap();
        let base_body = base.try_to_bytes().unwrap();
        let direct_body = direct.try_to_bytes().unwrap();
        let billboard_body = billboard.try_to_bytes().unwrap();
        let animated_billboard_body = animated_billboard.try_to_bytes().unwrap();
        assert_eq!(
            u32::from_le_bytes(base_body[0..4].try_into().unwrap()),
            SH_VOLUME_VERSION,
        );
        assert_eq!(
            u32::from_le_bytes(direct_body[0..4].try_into().unwrap()),
            DIRECT_SH_VOLUME_VERSION,
        );
        let emission =
            FinalizedShEmissionView::new(&base, Some(&direct), None, None, None, None, None, None)
                .unwrap();
        let sources = FinalizedShPackSources::new(&raw_base, Some(&raw_direct)).unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let output = tempdir.path().join("fixture.prl");
        let spool = build_cluster_payload_spool(&output, &directory, emission, sources).unwrap();
        super::super::write_and_validate_sections(
            &output,
            vec![
                planned_legacy_section(SectionId::OctahedralShVolume, 1, base_body.clone()),
                planned_legacy_section(SectionId::DirectShVolume, 1, direct_body.clone()),
                planned_legacy_section(
                    SectionId::BillboardDirectScatterVolume,
                    1,
                    billboard_body.clone(),
                ),
                planned_legacy_section(
                    SectionId::AnimatedBillboardDirectScatterDeltaVolumes,
                    1,
                    animated_billboard_body.clone(),
                ),
                planned_legacy_section(
                    SectionId::ClusterDirectory,
                    CLUSTER_DIRECTORY_CONTAINER_VERSION,
                    directory_body.clone(),
                ),
                spool.into_planned_section().unwrap(),
            ],
        )
        .unwrap();
        let mut file = std::fs::File::open(&output).unwrap();
        let meta = postretro_level_format::read_container(&mut file).unwrap();
        assert_eq!(
            meta.sections
                .iter()
                .map(|section| (section.section_id, section.version))
                .collect::<Vec<_>>(),
            vec![
                (SectionId::OctahedralShVolume as u32, 1),
                (SectionId::DirectShVolume as u32, 1),
                (SectionId::BillboardDirectScatterVolume as u32, 1),
                (
                    SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32,
                    1,
                ),
                (
                    SectionId::ClusterDirectory as u32,
                    CLUSTER_DIRECTORY_CONTAINER_VERSION,
                ),
                (
                    SectionId::ClusterShPayloads as u32,
                    CLUSTER_SH_PAYLOADS_CONTAINER_VERSION
                ),
            ],
        );
        for (section_id, expected) in [
            (SectionId::OctahedralShVolume, base_body),
            (SectionId::DirectShVolume, direct_body),
            (SectionId::BillboardDirectScatterVolume, billboard_body),
            (
                SectionId::AnimatedBillboardDirectScatterDeltaVolumes,
                animated_billboard_body,
            ),
            (SectionId::ClusterDirectory, directory_body),
        ] {
            let actual =
                postretro_level_format::read_section_data(&mut file, &meta, section_id as u32)
                    .unwrap()
                    .unwrap();
            assert_eq!(
                actual, expected,
                "legacy id {} body changed",
                section_id as u32
            );
        }
    }

    #[test]
    fn cluster_payload_reencodes_raw_cells_when_legacy_atlases_are_bc6h() {
        let raw_base = raw_octahedral_source();
        let raw_direct = raw_direct_source(&raw_base);
        let emitted_base = crate::sh_bake::encode_sh_volume_section_bc6h(&raw_base, false);
        let emitted_direct = crate::direct_sh_bake::encode_direct_section_bc6h(&raw_direct, false);
        assert_eq!(raw_base.irradiance_format, IRRADIANCE_FORMAT_RGBA16F);
        assert_eq!(raw_direct.irradiance_format, IRRADIANCE_FORMAT_RGBA16F);
        assert_eq!(emitted_base.irradiance_format, IRRADIANCE_FORMAT_BC6H);
        assert_eq!(emitted_direct.irradiance_format, IRRADIANCE_FORMAT_BC6H);

        let directory = directory(true, false);
        let emission = FinalizedShEmissionView::new(
            &emitted_base,
            Some(&emitted_direct),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let sources = FinalizedShPackSources::new(&raw_base, Some(&raw_direct)).unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let spool = build_cluster_payload_spool(
            &tempdir.path().join("bc6h.prl"),
            &directory,
            emission,
            sources,
        )
        .unwrap();
        let metadata_sources =
            source_metadata_from_sections(&emitted_base, Some(&emitted_direct), None, None, None);
        let inputs = ClusterShPayloadsValidationInputs {
            directory: &directory,
            base: ClusterShPayloadsBaseMetadata::from(&emitted_base),
            sources: &metadata_sources,
        };
        let decoded = spool
            .section
            .decode_chunk(0, std::fs::read(spool.spool.path()).unwrap(), inputs)
            .unwrap();

        assert_eq!(spool.section.index[0].stored_tile_count, 64);
        for section_id in [
            SectionId::OctahedralShVolume as u32,
            SectionId::DirectShVolume as u32,
        ] {
            let atlas = decoded
                .blocks
                .iter()
                .find(|block| {
                    block.section_id == section_id && block.kind == BLOCK_KIND_ISOLATED_ATLAS
                })
                .unwrap();
            let body = &decoded.bytes[atlas.body.clone()];
            assert_eq!(
                u32::from_le_bytes(body[0..4].try_into().unwrap()),
                IRRADIANCE_FORMAT_BC6H
            );
            assert_eq!(u32::from_le_bytes(body[4..8].try_into().unwrap()), 64);
            assert_eq!(
                body.len() - 20,
                64 * 4 * 16,
                "each isolated 8x8 cell must be four 16-byte BC6H blocks",
            );
        }
    }

    #[test]
    fn isolated_cells_dilate_two_right_and_bottom_texels() {
        let base = raw_octahedral_source();
        let directory = directory(false, false);
        let emission =
            FinalizedShEmissionView::new(&base, None, None, None, None, None, None, None).unwrap();
        let sources = FinalizedShPackSources::new(&base, None).unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let spool = build_cluster_payload_spool(
            &tempdir.path().join("fixture.prl"),
            &directory,
            emission,
            sources,
        )
        .unwrap();
        let metadata_sources = source_metadata_from_sections(&base, None, None, None, None);
        let inputs = ClusterShPayloadsValidationInputs {
            directory: &directory,
            base: ClusterShPayloadsBaseMetadata::from(&base),
            sources: &metadata_sources,
        };
        let decoded = spool
            .section
            .decode_chunk(0, std::fs::read(spool.spool.path()).unwrap(), inputs)
            .unwrap();
        let atlas = decoded
            .blocks
            .iter()
            .find(|block| {
                block.section_id == SectionId::OctahedralShVolume as u32
                    && block.kind == BLOCK_KIND_ISOLATED_ATLAS
            })
            .unwrap();
        let body = &decoded.bytes[atlas.body.clone()];
        let width = u32::from_le_bytes(body[8..12].try_into().unwrap()) as usize;
        let payload = &body[20..];
        assert_eq!(&payload[6 * 8..7 * 8], &payload[5 * 8..6 * 8]);
        let row_six = 6 * width * 8;
        let row_five = 5 * width * 8;
        assert_eq!(
            &payload[row_six..row_six + 8],
            &payload[row_five..row_five + 8]
        );
    }
}
