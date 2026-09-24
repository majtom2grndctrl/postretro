//! Small on-disk PRL for the controller's retained-manifest lifecycle test.

use postretro_level_format::bvh::BvhSection;
use postretro_level_format::cell_locator::{CellLocatorChild, CellLocatorSection};
use postretro_level_format::cells::{CellRecord, CellsSection};
use postretro_level_format::cluster_directory::{
    CLUSTER_DIRECTORY_CONTAINER_VERSION, ClusterDirectorySection, ClusterDirectoryShInventory,
    ClusterDirectoryValidationInputs, canonical_cell_partition, populate_canonical_resource_ranges,
};
use postretro_level_format::cluster_sh_payloads::{
    CLUSTER_SH_PAYLOADS_CONTAINER_VERSION, CLUSTER_SH_PAYLOADS_VERSION, ClusterShPayloadsHeader,
    ClusterShPayloadsIndexRecord, ClusterShPayloadsSection, ClusterShPayloadsSourceRecord,
    ClusterShPayloadsValidationInputs, cluster_sh_isolated_atlas_array_layout,
    source_metadata_from_sections,
};
use postretro_level_format::fog_volumes::FogVolumesSection;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::octahedral::{MAX_SH_ATLAS_DIMENSION, irradiance_atlas_array_layout};
use postretro_level_format::sh_volume::{
    OCTAHEDRAL_PROBE_STRIDE, OctahedralShProbe, OctahedralShVolumeSection,
};
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::{SectionBlob, SectionId, write_prl};

pub(super) fn write_one_cluster_prl() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("sync-proof.prl");
    let cells = CellsSection {
        cells: vec![CellRecord {
            bounds_min: [-1.0; 3],
            bounds_max: [1.0; 3],
            flags: 0,
            face_start: 0,
            face_count: 0,
            portal_ref_start: 0,
            portal_ref_count: 0,
        }],
        portal_refs: Vec::new(),
    };
    let bvh = BvhSection {
        nodes: Vec::new(),
        leaves: Vec::new(),
        root_node_index: 0,
    };
    let portals = postretro_level_format::portals::PortalsSection {
        vertices: Vec::new(),
        portals: Vec::new(),
    };
    let locator = CellLocatorSection {
        root: CellLocatorChild::Cell(0),
        nodes: Vec::new(),
    };
    let legacy_layout =
        irradiance_atlas_array_layout([1, 1, 1], 6, MAX_SH_ATLAS_DIMENSION).unwrap();
    let base = OctahedralShVolumeSection {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: [1, 1, 1],
        probe_stride: OCTAHEDRAL_PROBE_STRIDE,
        tile_dimension: 6,
        tile_border: 1,
        atlas_dimensions: [legacy_layout.atlas_width, legacy_layout.atlas_height],
        layer_count: legacy_layout.layer_count,
        tiles_per_layer: legacy_layout.tiles_per_layer,
        atlas_tiles_per_row: legacy_layout.atlas_tiles_per_row,
        probes: vec![OctahedralShProbe {
            validity: 1,
            mean_distance: 0x3c00,
            mean_sq_distance: 0x4000,
            density_level: 0,
            node_scale: 0,
        }],
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        compact_atlas: vec![
            0;
            legacy_layout.layer_count as usize
                * legacy_layout.atlas_width as usize
                * legacy_layout.atlas_height as usize
                * 8
        ],
        animation_descriptors: Vec::new(),
        slot_for_map_light: Vec::new(),
    };
    let mut directory = ClusterDirectorySection {
        runtime_cell_count: 1,
        primitive_limit: 64,
        cell_limit: 32,
        clusters: canonical_cell_partition(&cells, &portals, &bvh, 64, 32, &[])
            .unwrap()
            .clusters,
        resources: Vec::new(),
        members: vec![0],
        ranges: Vec::new(),
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    };
    let directory_inputs = ClusterDirectoryValidationInputs {
        cells: &cells,
        portals: &portals,
        bvh: &bvh,
        cell_locator: &locator,
        sh: ClusterDirectoryShInventory {
            octahedral: Some(&base),
            ..Default::default()
        },
    };
    populate_canonical_resource_ranges(&mut directory, directory_inputs).unwrap();
    directory.validate_semantics(directory_inputs).unwrap();

    let isolated = cluster_sh_isolated_atlas_array_layout(1).unwrap();
    let atlas_bytes = isolated.layer_count as usize
        * isolated.atlas_width as usize
        * isolated.atlas_height as usize
        * 8;
    let mut chunk = Vec::new();
    for word in [CLUSTER_SH_PAYLOADS_VERSION, 0, 2, 0] {
        push_u32(&mut chunk, word);
    }
    // Probe-patch block, then one independently addressable 8x8 atlas cell.
    for word in [SectionId::OctahedralShVolume as u32, 0, 1, 0] {
        push_u32(&mut chunk, word);
    }
    push_u64(&mut chunk, 80);
    push_u64(&mut chunk, 16);
    for word in [SectionId::OctahedralShVolume as u32, 1, 1, 0] {
        push_u32(&mut chunk, word);
    }
    push_u64(&mut chunk, 96);
    push_u64(&mut chunk, (20 + atlas_bytes) as u64);
    push_u32(&mut chunk, 0);
    push_u32(&mut chunk, 4);
    chunk.extend_from_slice(&0x3c00u16.to_le_bytes());
    chunk.extend_from_slice(&0x4000u16.to_le_bytes());
    push_u32(&mut chunk, 0);
    for word in [
        IRRADIANCE_FORMAT_RGBA16F,
        1,
        isolated.atlas_width,
        isolated.atlas_height,
        isolated.layer_count,
    ] {
        push_u32(&mut chunk, word);
    }
    chunk.resize(chunk.len() + atlas_bytes, 0);

    let sources = source_metadata_from_sections(&base, None, None, None, None);
    let payloads = ClusterShPayloadsSection {
        header: ClusterShPayloadsHeader {
            cluster_count: 1,
            source_count: 1,
            grid_dimensions: [1, 1, 1],
            affinity_dimensions: [1, 1, 1],
            payload_bytes: chunk.len() as u64,
        },
        sources: sources
            .iter()
            .map(|source| ClusterShPayloadsSourceRecord {
                section_id: source.section_id(),
                internal_version: source.internal_version(),
                kind: source.kind(),
            })
            .collect(),
        index: vec![ClusterShPayloadsIndexRecord {
            payload_offset: 0,
            payload_len: chunk.len() as u64,
            decoded_bytes: (16 + 20 + atlas_bytes) as u64,
            requested_resident_bytes: atlas_bytes as u64,
            stored_tile_count: 1,
            dense_patch_count: 1,
            affinity_patch_count: 0,
            hash: *blake3::hash(&chunk).as_bytes(),
        }],
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
            1,
            GeometrySection {
                vertices: Vec::new(),
                indices: Vec::new(),
                faces: Vec::new(),
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
        blob(SectionId::OctahedralShVolume, 1, base.to_bytes()),
        blob(
            SectionId::ClusterDirectory,
            CLUSTER_DIRECTORY_CONTAINER_VERSION,
            directory.try_to_bytes().unwrap(),
        ),
        blob(
            SectionId::ClusterShPayloads,
            CLUSTER_SH_PAYLOADS_CONTAINER_VERSION,
            payloads.try_to_bytes(&chunk).unwrap(),
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
