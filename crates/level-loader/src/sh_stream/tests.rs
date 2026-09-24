//! See: context/lib/testing_guide.md.

use std::fs::File;
use std::sync::Arc;

use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_format::direct_sh_delta_volumes::DIRECT_SH_DELTA_VOLUMES_VERSION;
use postretro_level_format::{ContainerMeta, SectionEntry, SectionId};

use super::boundary::{sorted_lists_intersect, validate_sorted_cluster_ids};
use super::manifest::{load_manifest_positionally, streaming_content_tag};
use super::metadata_base::{parse_animation_tail, read_base_metadata};
use super::metadata_sparse::{read_optional_direct_metadata, read_optional_sparse_metadata};
use super::positional_io::read_vec_at;
use super::*;

fn empty_directory() -> ClusterDirectorySection {
    ClusterDirectorySection {
        runtime_cell_count: 0,
        primitive_limit: 1,
        cell_limit: 1,
        clusters: Vec::new(),
        resources: Vec::new(),
        members: Vec::new(),
        ranges: Vec::new(),
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    }
}

#[test]
fn drain_accepts_dependent_first_evictions_but_rejects_duplicate_ids() {
    let batch = ShDrainBatch {
        generation: 1,
        content_tag: [7; 32],
        evictions: vec![1, 0],
        ..ShDrainBatch::default()
    };
    batch
        .validate_contract(2, [7; 32])
        .expect("dependent-before-owner eviction order is valid");
    let duplicate = ShDrainBatch {
        evictions: vec![1, 1],
        ..batch
    };
    assert!(duplicate.validate_contract(2, [7; 32]).is_err());
}

fn entry(section_id: SectionId, size: u64) -> SectionEntry {
    entry_at(section_id, 0, size)
}

fn entry_at(section_id: SectionId, offset: u64, size: u64) -> SectionEntry {
    SectionEntry {
        section_id: section_id as u32,
        offset,
        size,
        version: 1,
    }
}

fn container_with_entry(entry: SectionEntry) -> ContainerMeta {
    ContainerMeta {
        header: postretro_level_format::Header {
            version: postretro_level_format::CURRENT_VERSION,
            section_count: 1,
        },
        sections: vec![entry],
    }
}

fn base_metadata_for_sparse_header_checks() -> ShStreamBaseMetadata {
    ShStreamBaseMetadata {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: [4; 3],
        probe_stride: 8,
        tile_dimension: 4,
        tile_border: 1,
        atlas_dimensions: [8, 8],
        layer_count: 1,
        tiles_per_layer: 1,
        atlas_tiles_per_row: 1,
        irradiance_format: 0,
        probes: Vec::new(),
        animation_descriptors: Vec::new(),
        slot_for_map_light: Vec::new(),
    }
}

fn id50_header(source_count: u32) -> Vec<u8> {
    let mut bytes = vec![0; 72];
    bytes[0..4].copy_from_slice(&1u32.to_le_bytes());
    bytes[8..12].copy_from_slice(&source_count.to_le_bytes());
    bytes[40..44].copy_from_slice(&6u32.to_le_bytes());
    bytes[44..48].copy_from_slice(&1u32.to_le_bytes());
    bytes[48..52].copy_from_slice(&8u32.to_le_bytes());
    bytes
}

fn container_with_payload_offset(offset: u64) -> ContainerMeta {
    ContainerMeta {
        header: postretro_level_format::Header {
            version: postretro_level_format::CURRENT_VERSION,
            section_count: 2,
        },
        sections: vec![
            SectionEntry {
                section_id: SectionId::ClusterDirectory as u32,
                offset: 52,
                size: 16,
                version: 1,
            },
            SectionEntry {
                section_id: SectionId::ClusterShPayloads as u32,
                offset,
                size: 64,
                version: 1,
            },
        ],
    }
}

#[test]
fn content_tag_binds_ordered_container_table_identity() {
    let directory = b"directory bytes";
    let metadata = b"id50 metadata";
    let original = streaming_content_tag(&container_with_payload_offset(68), directory, metadata);
    let replaced_table =
        streaming_content_tag(&container_with_payload_offset(69), directory, metadata);
    assert_ne!(original, replaced_table);
}

#[cfg(unix)]
#[test]
fn retained_file_reads_original_bytes_after_path_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("map.prl");
    let replacement = directory.path().join("replacement.prl");
    std::fs::write(&path, b"original").unwrap();
    let file = File::open(&path).unwrap();
    std::fs::write(&replacement, b"replaced").unwrap();
    std::fs::rename(&replacement, &path).unwrap();

    assert_eq!(read_vec_at(&file, 0, 8, "test").unwrap(), b"original");
    assert_eq!(std::fs::read(path).unwrap(), b"replaced");
}

#[test]
fn drain_batch_rejects_unsorted_cluster_deltas() {
    assert!(validate_sorted_cluster_ids(&[1, 1], 3, "target-add").is_err());
    assert!(validate_sorted_cluster_ids(&[3], 3, "target-add").is_err());
    assert!(validate_sorted_cluster_ids(&[0, 2], 3, "target-add").is_ok());
    assert!(sorted_lists_intersect(&[0, 2], &[1, 2]));
    assert!(!sorted_lists_intersect(&[0], &[1, 2]));
}

#[test]
fn id50_metadata_length_is_bounded_before_the_variable_read() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("id50.prl");
    let offset = 30;
    let mut bytes = vec![0; offset];
    bytes.extend(id50_header(1));
    std::fs::write(&path, bytes).unwrap();
    let file = Arc::new(File::open(&path).unwrap());
    let error = load_manifest_positionally(
        file,
        path,
        container_with_entry(entry_at(SectionId::ClusterShPayloads, offset as u64, 72)),
        empty_directory(),
        &[],
    )
    .unwrap_err();
    assert!(
        format!("{error}").contains("id-50 metadata requires 88 bytes"),
        "unexpected error: {error}"
    );
}

#[test]
fn id50_source_count_cap_is_checked_before_metadata_allocation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("id50.prl");
    let offset = 30;
    let mut bytes = vec![0; offset];
    bytes.extend(id50_header(u32::MAX));
    std::fs::write(&path, bytes).unwrap();
    let file = Arc::new(File::open(&path).unwrap());
    let error = load_manifest_positionally(
        file,
        path,
        container_with_entry(entry_at(SectionId::ClusterShPayloads, offset as u64, 72)),
        empty_directory(),
        &[],
    )
    .unwrap_err();
    assert!(
        format!("{error}").contains("source count"),
        "unexpected error: {error}"
    );
}

#[test]
fn fixed_metadata_headers_are_bounded_before_positional_reads() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("short.prl");
    std::fs::write(&path, vec![0; 84]).unwrap();
    let file = File::open(&path).unwrap();

    assert!(read_base_metadata(&file, &entry(SectionId::OctahedralShVolume, 83)).is_err());
    assert!(
        read_optional_direct_metadata(&file, Some(&entry(SectionId::DirectShVolume, 75)),).is_err()
    );
    assert!(
        read_optional_sparse_metadata(
            &file,
            Some(&entry(SectionId::DirectShDeltaVolumes, 21)),
            SectionId::DirectShDeltaVolumes,
            &base_metadata_for_sparse_header_checks(),
        )
        .is_err()
    );
}

#[test]
fn sparse_header_geometry_is_checked_before_variable_tables() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sparse.prl");
    let mut header = vec![0; 22];
    header[0] = DIRECT_SH_DELTA_VOLUMES_VERSION;
    header[1] = postretro_level_format::delta_sh_volumes::AFFINITY_FACTOR;
    header[2..6].copy_from_slice(&u32::MAX.to_le_bytes());
    header[6..10].copy_from_slice(&u32::MAX.to_le_bytes());
    header[10..14].copy_from_slice(&u32::MAX.to_le_bytes());
    header[14..18].copy_from_slice(&4u32.to_le_bytes());
    header[18..22].copy_from_slice(&1u32.to_le_bytes());
    std::fs::write(&path, header).unwrap();

    let error = read_optional_sparse_metadata(
        &File::open(&path).unwrap(),
        Some(&entry(SectionId::DirectShDeltaVolumes, 22)),
        SectionId::DirectShDeltaVolumes,
        &base_metadata_for_sparse_header_checks(),
    )
    .unwrap_err();
    assert!(format!("{error}").contains("does not match id-34"));
}

#[test]
fn animation_count_is_bounded_by_the_metadata_tail_before_allocation() {
    let error = parse_animation_tail(&[], u32::MAX).unwrap_err();
    assert!(format!("{error}").contains("descriptor headers are truncated"));
}
