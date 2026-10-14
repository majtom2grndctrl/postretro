//! Tiny published PRL for the real loader → manifest → GPU growth proof.
use postretro_level_format::bvh::BvhSection;
use postretro_level_format::cell_locator::{
    CellLocatorChild, CellLocatorNodeRecord, CellLocatorSection,
};
use postretro_level_format::cells::{CellRecord, CellsSection};
use postretro_level_format::cluster_directory::{
    CLUSTER_DIRECTORY_CONTAINER_VERSION, ClusterDirectorySection, ClusterDirectoryShInventory,
    ClusterDirectoryValidationInputs, canonical_cell_partition, populate_canonical_resource_ranges,
};
use postretro_level_format::cluster_sh_payloads::*;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::fog_volumes::FogVolumesSection;
use postretro_level_format::geometry::{GEOMETRY_CONTAINER_VERSION, GeometrySection};
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::octahedral::{MAX_SH_ATLAS_DIMENSION, irradiance_atlas_array_layout};
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::sh_volume::{
    OCTAHEDRAL_PROBE_STRIDE, OctahedralShProbe, OctahedralShVolumeSection,
};
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::{SectionBlob, SectionId, write_prl};

pub(super) fn write_nine_cluster_prl() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("zero-share-growth.prl");
    let cells = CellsSection {
        cells: (0..9)
            .map(|i| CellRecord {
                bounds_min: [i as f32 * 4.0, 0.0, 0.0],
                bounds_max: [(i + 1) as f32 * 4.0, 4.0, 4.0],
                flags: 0,
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
            })
            .collect(),
        portal_refs: vec![],
    };
    let bvh = BvhSection {
        nodes: vec![],
        leaves: vec![],
        root_node_index: 0,
    };
    let portals = PortalsSection {
        vertices: vec![],
        portals: vec![],
    };
    let locator = CellLocatorSection {
        root: CellLocatorChild::Node(0),
        nodes: (0..8)
            .map(|i| CellLocatorNodeRecord {
                plane_normal: [1.0, 0.0, 0.0],
                plane_distance: (i + 1) as f32 * 4.0,
                back: CellLocatorChild::Cell(i),
                front: if i == 7 {
                    CellLocatorChild::Cell(8)
                } else {
                    CellLocatorChild::Node(i + 1)
                },
            })
            .collect(),
    };
    let legacy = irradiance_atlas_array_layout([576, 1, 1], 6, MAX_SH_ATLAS_DIMENSION).unwrap();
    let base = OctahedralShVolumeSection {
        grid_origin: [0.5; 3],
        cell_size: [1.0; 3],
        grid_dimensions: [36, 4, 4],
        probe_stride: OCTAHEDRAL_PROBE_STRIDE,
        tile_dimension: 6,
        tile_border: 1,
        atlas_dimensions: [legacy.atlas_width, legacy.atlas_height],
        layer_count: legacy.layer_count,
        tiles_per_layer: legacy.tiles_per_layer,
        atlas_tiles_per_row: legacy.atlas_tiles_per_row,
        probes: vec![
            OctahedralShProbe {
                validity: 1,
                density_level: 0,
                node_scale: 0,
                mean_distance: 0,
                mean_sq_distance: 0
            };
            576
        ],
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        compact_atlas: vec![
            0;
            legacy.layer_count as usize
                * legacy.atlas_width as usize
                * legacy.atlas_height as usize
                * 8
        ],
        animation_descriptors: vec![],
        slot_for_map_light: vec![],
    };
    // One cell per cluster guarantees the old one-writer layer cannot hold
    // this level at four layers, while each writer remains a whole node.
    let partition = canonical_cell_partition(&cells, &portals, &bvh, 64, 1, &[]).unwrap();
    let mut directory = ClusterDirectorySection {
        runtime_cell_count: 9,
        primitive_limit: 64,
        cell_limit: 1,
        clusters: partition.clusters,
        resources: vec![],
        members: partition.members,
        ranges: vec![],
        seam_portal_ids: vec![],
        cluster_hints: vec![],
    };
    let inputs = ClusterDirectoryValidationInputs {
        cells: &cells,
        portals: &portals,
        bvh: &bvh,
        cell_locator: &locator,
        sh: ClusterDirectoryShInventory {
            octahedral: Some(&base),
            ..Default::default()
        },
    };
    populate_canonical_resource_ranges(&mut directory, inputs).unwrap();
    directory.validate_semantics(inputs).unwrap();
    assert_eq!(directory.clusters.len(), 9);
    let projected = postretro_level_loader::ShStreamBaseMetadata {
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
        irradiance_format: base.irradiance_format,
        probes: base.probes.clone(),
        animation_descriptors: base.animation_descriptors.clone(),
        slot_for_map_light: base.slot_for_map_light.clone(),
    };
    let residency = crate::render::sh_streaming::ShResidencyState::from_parts(
        [7; 32],
        &directory,
        &projected,
        &postretro_level_loader::ShStreamSourceMetadata::default(),
        &[],
    )
    .unwrap();
    let mut payload = Vec::new();
    let mut index = Vec::new();
    for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
        // The canonical directory includes stencil halos and whole-node
        // expansion. Derive every count from that output, not a nominal box.
        let dense = directory.ranges
            [cluster.range_start as usize..(cluster.range_start + cluster.range_count) as usize]
            .iter()
            .filter(|range| directory.resources[range.resource_index as usize].section_id == 34)
            .flat_map(|range| range.start..range.start + range.count)
            .collect::<std::collections::BTreeSet<_>>();
        let mut nodes = dense
            .iter()
            .filter_map(|&probe| residency.dense_node[probe as usize])
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        nodes.sort_by_key(|node| residency.node_layouts[node].global_base_slot);
        let mut closure = std::collections::BTreeMap::new();
        let mut tiles = 0;
        for node in nodes {
            closure.insert(node, tiles);
            tiles += residency.node_layouts[&node].tile_count;
        }
        let owned_tiles: u32 = residency
            .nodes_by_owner
            .get(&(cluster_id as u32))
            .into_iter()
            .flatten()
            .map(|node| residency.node_layouts[node].tile_count)
            .sum();
        let isolated = cluster_sh_isolated_atlas_array_layout(tiles).unwrap();
        let atlas_bytes = isolated.layer_count as usize
            * isolated.atlas_width as usize
            * isolated.atlas_height as usize
            * 8;
        let patches = dense.len() as u32;
        let mut chunk = Vec::new();
        for word in [CLUSTER_SH_PAYLOADS_VERSION, cluster_id as u32, 2, 0] {
            push_u32(&mut chunk, word);
        }
        for word in [34, 0, patches, 0] {
            push_u32(&mut chunk, word);
        }
        push_u64(&mut chunk, 80);
        push_u64(&mut chunk, u64::from(patches) * 16);
        for word in [34, 1, tiles, 0] {
            push_u32(&mut chunk, word);
        }
        push_u64(&mut chunk, 80 + u64::from(patches) * 16);
        push_u64(&mut chunk, (20 + atlas_bytes) as u64);
        for dense in dense {
            let node = residency.dense_node[dense as usize].unwrap();
            let local = closure[&node] + residency.dense_node_local_slot[dense as usize].unwrap();
            for word in [dense, 4 | (local << 5), 0, 0] {
                push_u32(&mut chunk, word);
            }
        }
        for word in [
            IRRADIANCE_FORMAT_RGBA16F,
            tiles,
            isolated.atlas_width,
            isolated.atlas_height,
            isolated.layer_count,
        ] {
            push_u32(&mut chunk, word);
        }
        chunk.resize(chunk.len() + atlas_bytes, 0);
        index.push(ClusterShPayloadsIndexRecord {
            payload_offset: payload.len() as u64,
            payload_len: chunk.len() as u64,
            decoded_bytes: (patches as usize * 16 + 20 + atlas_bytes) as u64,
            requested_resident_bytes: u64::from(owned_tiles) * 512,
            stored_tile_count: tiles,
            dense_patch_count: patches,
            affinity_patch_count: 0,
            hash: *blake3::hash(&chunk).as_bytes(),
        });
        payload.extend_from_slice(&chunk);
    }
    let sources = source_metadata_from_sections(&base, None, None, None, None);
    let payloads = ClusterShPayloadsSection {
        header: ClusterShPayloadsHeader {
            cluster_count: 9,
            source_count: 1,
            grid_dimensions: base.grid_dimensions,
            affinity_dimensions: [9, 1, 1],
            payload_bytes: payload.len() as u64,
        },
        sources: sources
            .iter()
            .map(|source| ClusterShPayloadsSourceRecord {
                section_id: source.section_id(),
                internal_version: source.internal_version(),
                kind: source.kind(),
            })
            .collect(),
        index,
    };
    payloads
        .validate_against(ClusterShPayloadsValidationInputs {
            directory: &directory,
            base: (&base).into(),
            sources: &sources,
        })
        .unwrap();
    let blob = |section_id: SectionId, version, data| SectionBlob {
        section_id: section_id as u32,
        version,
        data,
    };
    let sections = vec![
        blob(
            SectionId::Geometry,
            GEOMETRY_CONTAINER_VERSION,
            GeometrySection {
                vertices: vec![],
                indices: vec![],
                faces: vec![],
            }
            .to_bytes(),
        ),
        blob(SectionId::Bvh, 1, bvh.to_bytes()),
        blob(SectionId::Cells, 1, cells.to_bytes()),
        blob(SectionId::CellLocator, 1, locator.to_bytes()),
        blob(
            SectionId::FogVolumes,
            1,
            FogVolumesSection::default().to_bytes(),
        ),
        blob(
            SectionId::EnvironmentVolumes,
            1,
            EnvironmentVolumesSection::default().to_bytes(),
        ),
        blob(SectionId::OctahedralShVolume, 1, base.to_bytes()),
        blob(
            SectionId::ClusterDirectory,
            CLUSTER_DIRECTORY_CONTAINER_VERSION,
            directory.try_to_bytes().unwrap(),
        ),
        blob(
            SectionId::ClusterShPayloads,
            CLUSTER_SH_PAYLOADS_CONTAINER_VERSION,
            payloads.try_to_bytes(&payload).unwrap(),
        ),
        blob(
            SectionId::TextureCacheKeys,
            1,
            TextureCacheKeysSection::default().to_bytes(),
        ),
    ];
    write_prl(&mut std::fs::File::create(&path).unwrap(), &sections).unwrap();
    (temp, path)
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
