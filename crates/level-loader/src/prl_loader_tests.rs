// Unit tests for PRL section decoding and cross-validation (`prl_loader`).
// See: context/lib/testing_guide.md

use super::*;
use crate::prl_load_test_fixtures::write_prl_load_fixture;
use log::Level;
use postretro_level_format::alpha_lights::AlphaLightRecord;
use postretro_level_format::cell_visibility::CoupledPairRecord;
use postretro_level_format::cluster_directory::{
    ClusterRecord, ClusterResourceDomain, ClusterResourceRecord,
};
use postretro_level_format::geometry::{FaceMeta as PrlFaceMeta, Vertex as PrlVertex};
use postretro_level_format::kinematic_geometry::{
    KINEMATIC_GEOMETRY_VERSION, KINEMATIC_GEOMETRY_VERSION_V4, KINEMATIC_GEOMETRY_VERSION_V5,
    KINEMATIC_GEOMETRY_VERSION_V6, KinematicMoverRecord, KinematicWaypointRecord, MemberLight,
};
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_test_log_capture::LogCapture;
use std::io::Cursor;

#[test]
fn cell_visibility_section_round_trips_through_runtime_lowering() {
    let encoded = CellVisibilitySection {
        cell_count: 3,
        component_ids: vec![0, 0, 1],
        coupled_pairs: vec![CoupledPairRecord {
            cell_a: 0,
            cell_b: 1,
            distance: 128,
            aperture: 64,
        }],
    }
    .to_bytes()
    .unwrap();
    let parsed = CellVisibilitySection::from_bytes(&encoded, 3).unwrap();
    let visibility = convert_cell_visibility_section(parsed);

    assert_eq!(visibility.component_ids(), &[0, 0, 1]);
    assert_eq!(
        visibility.coupled_pairs().copied().collect::<Vec<_>>(),
        vec![CoupledCellPair {
            cell_a: 0,
            cell_b: 1,
            distance: 128,
            aperture: 64,
        }]
    );

    let component_only = convert_cell_visibility_section(CellVisibilitySection {
        cell_count: 3,
        component_ids: vec![0, 0, 1],
        coupled_pairs: vec![],
    });
    assert_eq!(component_only.component_ids().len(), 3);
    assert_eq!(component_only.coupled_pairs().count(), 0);
}

#[test]
fn id50_without_id49_is_rejected_before_off_mode_can_select_legacy_loading() {
    let path = write_prl_load_fixture(
        [prl_format::SectionBlob {
            section_id: SectionId::ClusterShPayloads as u32,
            version:
                postretro_level_format::cluster_sh_payloads::CLUSTER_SH_PAYLOADS_CONTAINER_VERSION,
            // The id-50/id-49 pair is mandatory before the injected Off
            // mode can select retained-handle legacy loading. The
            // end-to-end streaming fixture separately covers malformed
            // id-50 bytes with a valid id-49 directory.
            data: vec![0],
        }],
        "postretro_test_invalid_id50_before_off.prl",
    );
    let error = load_prl_with_streaming_mode_for_test(path.to_str().unwrap(), ShStreamingMode::Off)
        .unwrap_err();
    assert!(matches!(error, PrlLoadError::ClusterShPayloads(_)));
    let _ = std::fs::remove_file(path);
}

#[test]
fn legacy_off_mode_soft_disables_an_out_of_bounds_optional_scatter_entry() {
    let path = write_prl_load_fixture(
        [prl_format::SectionBlob {
            section_id: SectionId::BillboardDirectScatterVolume as u32,
            version: 1,
            data: vec![0],
        }],
        "postretro_test_legacy_optional_scatter_out_of_bounds.prl",
    );
    let mut bytes = std::fs::read(&path).unwrap();
    let section_count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    let table_entry = (0..section_count)
        .map(|index| 8 + index * 22)
        .find(|&offset| {
            u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
                == SectionId::BillboardDirectScatterVolume as u32
        })
        .expect("fixture contains id 47");
    let out_of_bounds = u64::try_from(bytes.len()).unwrap() + 1;
    bytes[table_entry + 4..table_entry + 12].copy_from_slice(&out_of_bounds.to_le_bytes());
    std::fs::write(&path, bytes).unwrap();

    let loaded =
        load_prl_with_streaming_mode_for_test(path.to_str().unwrap(), ShStreamingMode::Off)
            .expect("legacy/off loading soft-disables malformed optional scatter");
    assert!(loaded.billboard_direct_scatter_volume().is_none());
    let _ = std::fs::remove_file(path);
}

fn write_cell_visibility_load_fixture(
    section: Option<prl_format::SectionBlob>,
    name: &str,
) -> std::path::PathBuf {
    write_prl_load_fixture(section, name)
}

fn zero_grid_cluster_directory(resource_ids: &[SectionId]) -> ClusterDirectorySection {
    ClusterDirectorySection {
        runtime_cell_count: 2,
        primitive_limit: 64,
        cell_limit: 64,
        clusters: vec![
            ClusterRecord {
                bounds_min: [0.0, 0.0, 0.0],
                bounds_max: [1.0, 1.0, 1.0],
                member_start: 0,
                member_count: 1,
                range_start: 0,
                range_count: 0,
                primitive_count: 0,
                flags: 0,
            },
            ClusterRecord {
                bounds_min: [2.0, 0.0, 0.0],
                bounds_max: [3.0, 1.0, 1.0],
                member_start: 1,
                member_count: 1,
                range_start: 0,
                range_count: 0,
                primitive_count: 0,
                flags: 0,
            },
        ],
        resources: resource_ids
            .iter()
            .map(|&section| ClusterResourceRecord {
                section_id: section as u32,
                domain: match section {
                    SectionId::OctahedralShVolume
                    | SectionId::DirectShVolume
                    | SectionId::BillboardDirectScatterVolume => ClusterResourceDomain::DenseProbe,
                    _ => ClusterResourceDomain::AffinityCell,
                },
                dimensions: [0, 0, 0],
            })
            .collect(),
        members: vec![0, 1],
        ranges: Vec::new(),
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    }
}

fn cluster_directory_blob(section: &ClusterDirectorySection) -> prl_format::SectionBlob {
    prl_format::SectionBlob {
        section_id: SectionId::ClusterDirectory as u32,
        version: CLUSTER_DIRECTORY_CONTAINER_VERSION,
        data: section.try_to_bytes().unwrap(),
    }
}

#[test]
fn cluster_directory_missing_is_silent_and_valid_zero_grid_directory_loads_inert() {
    let missing_path = write_prl_load_fixture([], "postretro_test_cluster_directory_missing.prl");
    let missing = load_prl(missing_path.to_str().unwrap()).unwrap();
    assert!(missing.cluster_directory.is_none());
    std::fs::remove_file(missing_path).unwrap();

    let directory = zero_grid_cluster_directory(&[SectionId::OctahedralShVolume]);
    let path = write_prl_load_fixture(
        [cluster_directory_blob(&directory)],
        "postretro_test_cluster_directory_zero_grid.prl",
    );
    let loaded = load_prl(path.to_str().unwrap()).unwrap();
    assert_eq!(loaded.cluster_directory.as_ref(), Some(&directory));
    assert!(loaded.sh_volume.is_some());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn cluster_directory_rejects_duplicate_versions_corruption_and_absent_targets() {
    let directory = zero_grid_cluster_directory(&[SectionId::OctahedralShVolume]);
    let duplicate_path = write_prl_load_fixture(
        [
            cluster_directory_blob(&directory),
            cluster_directory_blob(&directory),
        ],
        "postretro_test_cluster_directory_duplicate.prl",
    );
    assert!(matches!(
        load_prl(duplicate_path.to_str().unwrap()),
        Err(PrlLoadError::ClusterDirectory(
            ClusterDirectoryError::InvalidData(_)
        ))
    ));
    std::fs::remove_file(duplicate_path).unwrap();

    let mut v1_internal = cluster_directory_blob(&directory);
    v1_internal.data[0..4].copy_from_slice(&1u32.to_le_bytes());
    let internal_version_path = write_prl_load_fixture(
        [v1_internal],
        "postretro_test_cluster_directory_internal_version.prl",
    );
    assert!(matches!(
        load_prl(internal_version_path.to_str().unwrap()),
        Err(PrlLoadError::ClusterDirectory(
            ClusterDirectoryError::VersionMismatch { .. }
        ))
    ));
    std::fs::remove_file(internal_version_path).unwrap();

    let mut wrong_container = cluster_directory_blob(&directory);
    wrong_container.version = 1;
    let version_path = write_prl_load_fixture(
        [wrong_container],
        "postretro_test_cluster_directory_container_version.prl",
    );
    assert!(matches!(
        load_prl(version_path.to_str().unwrap()),
        Err(PrlLoadError::ClusterDirectory(
            ClusterDirectoryError::VersionMismatch { .. }
        ))
    ));
    std::fs::remove_file(version_path).unwrap();

    let corrupt_path = write_prl_load_fixture(
        [prl_format::SectionBlob {
            section_id: SectionId::ClusterDirectory as u32,
            version: CLUSTER_DIRECTORY_CONTAINER_VERSION,
            data: vec![0; 7],
        }],
        "postretro_test_cluster_directory_corrupt.prl",
    );
    assert!(matches!(
        load_prl(corrupt_path.to_str().unwrap()),
        Err(PrlLoadError::ClusterDirectory(
            ClusterDirectoryError::InvalidData(_)
        ))
    ));
    std::fs::remove_file(corrupt_path).unwrap();

    let absent_target =
        zero_grid_cluster_directory(&[SectionId::OctahedralShVolume, SectionId::DirectShVolume]);
    let absent_path = write_prl_load_fixture(
        [cluster_directory_blob(&absent_target)],
        "postretro_test_cluster_directory_absent_target.prl",
    );
    assert!(matches!(
        load_prl(absent_path.to_str().unwrap()),
        Err(PrlLoadError::ClusterDirectory(
            ClusterDirectoryError::MissingResource(_)
        ))
    ));
    std::fs::remove_file(absent_path).unwrap();
}

#[test]
fn cluster_directory_becomes_unavailable_when_present_delta_exceeds_policy_floor() {
    let delta_data = empty_delta_sh_section_bytes();
    let binding_floor = u64::try_from(delta_data.len() - 1).unwrap();
    let directory =
        zero_grid_cluster_directory(&[SectionId::DeltaShVolumes, SectionId::OctahedralShVolume]);
    let path = write_prl_load_fixture(
        [
            prl_format::SectionBlob {
                section_id: SectionId::DeltaShVolumes as u32,
                version: 1,
                data: delta_data,
            },
            cluster_directory_blob(&directory),
        ],
        "postretro_test_cluster_directory_policy_floor.prl",
    );
    let capture = LogCapture::start();
    let loaded = load_prl_with_delta_binding_limit(path.to_str().unwrap(), binding_floor)
        .expect("legacy over-floor degradation must remain a successful load");
    assert!(loaded.delta_sh_volumes.is_none());
    assert!(loaded.cluster_directory.is_none());
    capture.assert_logged_once(Level::Warn, "ClusterDirectoryUnavailableCompanion");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn cluster_directory_validates_present_empty_id27_with_production_codec() {
    let directory =
        zero_grid_cluster_directory(&[SectionId::DeltaShVolumes, SectionId::OctahedralShVolume]);
    let path = write_prl_load_fixture(
        [
            prl_format::SectionBlob {
                section_id: SectionId::DeltaShVolumes as u32,
                version: 1,
                data: empty_delta_sh_section_bytes(),
            },
            cluster_directory_blob(&directory),
        ],
        "postretro_test_cluster_directory_empty_id27.prl",
    );
    let loaded = load_prl(path.to_str().unwrap()).unwrap();
    assert!(loaded.delta_sh_volumes.is_some());
    assert_eq!(loaded.cluster_directory, Some(directory));
    std::fs::remove_file(path).unwrap();
}

fn mark_section_out_of_bounds_above_binding_floor(path: &std::path::Path, section_id: SectionId) {
    const PRL_HEADER_SIZE: usize = 8;
    const PRL_SECTION_ENTRY_SIZE: usize = 22;
    const SECTION_SIZE_OFFSET: usize = 12;

    let mut file_data = std::fs::read(path).unwrap();
    let mut cursor = std::io::Cursor::new(&file_data);
    let meta = prl_format::read_container(&mut cursor).unwrap();
    let section_index = meta
        .sections
        .iter()
        .position(|entry| entry.section_id == section_id as u32)
        .expect("fixture must contain the section it makes oversized");
    let size_start = PRL_HEADER_SIZE + section_index * PRL_SECTION_ENTRY_SIZE + SECTION_SIZE_OFFSET;
    file_data[size_start..size_start + std::mem::size_of::<u64>()]
        .copy_from_slice(&(MAX_DELTA_SECTION_BINDING_BYTES + 1).to_le_bytes());
    std::fs::write(path, file_data).unwrap();
}

fn static_alpha_lights_blob() -> prl_format::SectionBlob {
    prl_format::SectionBlob {
        section_id: SectionId::AlphaLights as u32,
        version: 1,
        data: AlphaLightsSection {
            lights: vec![AlphaLightRecord {
                origin: [0.0, 0.0, 0.0],
                light_type: AlphaLightType::Point,
                intensity: 1.0,
                color: [1.0, 1.0, 1.0],
                falloff_model: AlphaFalloffModel::Linear,
                falloff_range: 8.0,
                cone_angle_inner: 0.0,
                cone_angle_outer: 0.0,
                cone_direction: [0.0, 0.0, 0.0],
                is_dynamic: false,
                casts_entity_shadows: false,
                leaf_index: ALPHA_LIGHT_LEAF_UNASSIGNED,
                shadow_type: AlphaShadowType::StaticLightMap,
            }],
        }
        .to_bytes(),
    }
}

fn empty_delta_sh_section_bytes() -> Vec<u8> {
    let base = OctahedralShVolumeSection::placeholder();
    let affinity_dims = base
        .grid_dimensions
        .map(|dimension| dimension.div_ceil(AFFINITY_FACTOR as u32));
    let cell_count = affinity_dims.iter().product::<u32>() as usize;
    DeltaShVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims,
        tile_dimension: base.tile_dimension,
        tile_border: base.tile_border,
        animation_descriptor_indices: Vec::new(),
        valid_probe_masks: (0..cell_count)
            .map(|cell| valid_probe_mask_for_affinity_cell(&base, affinity_dims, cell))
            .collect(),
        cell_levels: vec![0; cell_count],
        affinity_offsets: vec![0; cell_count + 1],
        affinity_lights: Vec::new(),
        delta_subblocks: Vec::new(),
    }
    .to_bytes()
}

fn empty_animated_direct_sh_delta_section_bytes() -> Vec<u8> {
    let base = OctahedralShVolumeSection::placeholder();
    let affinity_dims = base
        .grid_dimensions
        .map(|dimension| dimension.div_ceil(AFFINITY_FACTOR as u32));
    let cell_count = affinity_dims.iter().product::<u32>() as usize;
    AnimatedDirectShDeltaVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims,
        tile_dimension: base.tile_dimension,
        tile_border: base.tile_border,
        animation_descriptor_indices: Vec::new(),
        valid_probe_masks: (0..cell_count)
            .map(|cell| valid_probe_mask_for_affinity_cell(&base, affinity_dims, cell))
            .collect(),
        cell_levels: vec![0; cell_count],
        affinity_offsets: vec![0; cell_count + 1],
        affinity_lights: Vec::new(),
        delta_subblocks: Vec::new(),
    }
    .to_bytes()
}

fn empty_direct_sh_delta_section_bytes() -> Vec<u8> {
    let base = OctahedralShVolumeSection::placeholder();
    let affinity_dims = base
        .grid_dimensions
        .map(|dimension| dimension.div_ceil(AFFINITY_FACTOR as u32));
    let cell_count = affinity_dims.iter().product::<u32>() as usize;
    DirectShDeltaVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims,
        tile_dimension: base.tile_dimension,
        tile_border: base.tile_border,
        valid_probe_masks: (0..cell_count)
            .map(|cell| valid_probe_mask_for_affinity_cell(&base, affinity_dims, cell))
            .collect(),
        cell_levels: vec![0; cell_count],
        affinity_offsets: vec![0; cell_count + 1],
        affinity_lights: Vec::new(),
        delta_subblocks: Vec::new(),
    }
    .to_bytes()
}

// Regression: oversized malformed metadata bypassed container-bounds validation.
#[test]
fn binding_floor_validates_each_delta_section_container_before_degrading() {
    for (section_id, name) in [
        (SectionId::DeltaShVolumes, "DeltaShVolumes"),
        (SectionId::DirectShDeltaVolumes, "DirectShDeltaVolumes"),
        (
            SectionId::AnimatedDirectShDeltaVolumes,
            "AnimatedDirectShDeltaVolumes",
        ),
    ] {
        let meta = prl_format::ContainerMeta {
            header: prl_format::Header {
                version: prl_format::CURRENT_VERSION,
                section_count: 1,
            },
            sections: vec![prl_format::SectionEntry {
                section_id: section_id as u32,
                offset: u64::MAX,
                size: MAX_DELTA_SECTION_BINDING_BYTES + 1,
                version: 1,
            }],
        };

        let container = PrlContainer::from_whole_bytes(Vec::new(), meta, None);
        let result = read_bounded_delta_section_data(&container, section_id, name);
        assert!(
            matches!(
                result,
                Err(PrlLoadError::FormatError(
                    prl_format::FormatError::SectionOffsetOverflow { .. }
                ))
            ),
            "{name} must reject invalid container bounds before applying the binding floor"
        );
    }
}

// Regression: valid over-floor payloads must retain the established degradation path.
#[test]
fn binding_floor_degrades_each_valid_oversized_delta_section_after_borrowing() {
    for (section_id, name) in [
        (SectionId::DeltaShVolumes, "DeltaShVolumes"),
        (SectionId::DirectShDeltaVolumes, "DirectShDeltaVolumes"),
        (
            SectionId::AnimatedDirectShDeltaVolumes,
            "AnimatedDirectShDeltaVolumes",
        ),
    ] {
        let mut file_data = Vec::new();
        prl_format::write_prl(
            &mut file_data,
            &[prl_format::SectionBlob {
                section_id: section_id as u32,
                version: 1,
                data: vec![0_u8; 2],
            }],
        )
        .expect("fixture container should serialize");
        let mut cursor = Cursor::new(&file_data);
        let meta = prl_format::read_container(&mut cursor)
            .expect("fixture container metadata should parse");
        let container = PrlContainer::from_whole_bytes(file_data, meta, None);

        let outcome = read_bounded_delta_section_data_with_limit(&container, section_id, name, 1)
            .expect("valid container bounds must reach the binding-floor policy");
        assert!(matches!(outcome, BoundedDeltaSectionData::OverBindingFloor));
    }
}

#[test]
fn load_prl_degrades_valid_over_floor_id27_and_id45_independently() {
    for (section_id, name, data) in [
        (
            SectionId::DeltaShVolumes,
            "DeltaShVolumes",
            empty_delta_sh_section_bytes(),
        ),
        (
            SectionId::AnimatedDirectShDeltaVolumes,
            "AnimatedDirectShDeltaVolumes",
            empty_animated_direct_sh_delta_section_bytes(),
        ),
    ] {
        let binding_floor = u64::try_from(data.len() - 1)
            .expect("fixture must fit the test-only binding-floor type");
        let path = write_prl_load_fixture(
            [prl_format::SectionBlob {
                section_id: section_id as u32,
                version: 1,
                data,
            }],
            &format!("postretro_test_{name}_binding_floor_degrade.prl"),
        );

        let world = load_prl_with_delta_binding_limit(path.to_str().unwrap(), binding_floor)
            .expect("a valid over-floor optional delta section must degrade, not fail loading");
        match section_id {
            SectionId::DeltaShVolumes => assert!(world.delta_sh_volumes.is_none()),
            SectionId::AnimatedDirectShDeltaVolumes => {
                assert!(world.animated_direct_sh_delta_volumes.is_none())
            }
            _ => unreachable!("table contains only independently degradable delta sections"),
        }
        std::fs::remove_file(path).ok();
    }
}

#[test]
fn load_prl_over_floor_id41_clears_paired_entity_shadow_selection() {
    let direct_delta = empty_direct_sh_delta_section_bytes();
    let binding_floor = u64::try_from(direct_delta.len() - 1)
        .expect("fixture must fit the test-only binding-floor type");
    let path = write_prl_load_fixture(
        [
            static_alpha_lights_blob(),
            prl_format::SectionBlob {
                section_id: SectionId::DirectShVolume as u32,
                version: 1,
                data: DirectShVolumeSection::placeholder().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::EntityShadowLights as u32,
                version: 1,
                data: EntityShadowLightsSection {
                    light_indices: vec![0],
                }
                .to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::DirectShDeltaVolumes as u32,
                version: 1,
                data: direct_delta,
            },
        ],
        "postretro_test_direct_sh_binding_floor_pair_clear.prl",
    );

    let world = load_prl_with_delta_binding_limit(path.to_str().unwrap(), binding_floor)
        .expect("an over-floor id-41 section must degrade through the load path");
    assert!(world.direct_sh_delta_volumes.is_none());
    assert!(world.entity_shadow_lights.is_empty());
    std::fs::remove_file(path).ok();
}

#[test]
fn out_of_bounds_id27_above_binding_floor_still_fails_load() {
    let oversized_path = write_cell_visibility_load_fixture(
        Some(prl_format::SectionBlob {
            section_id: SectionId::DeltaShVolumes as u32,
            version: 1,
            data: vec![0],
        }),
        "postretro_test_delta_sh_over_binding_floor.prl",
    );
    mark_section_out_of_bounds_above_binding_floor(&oversized_path, SectionId::DeltaShVolumes);

    let err = load_prl(oversized_path.to_str().unwrap()).unwrap_err();
    assert!(matches!(
        err,
        PrlLoadError::FormatError(prl_format::FormatError::SectionOutOfBounds { .. })
    ));
    std::fs::remove_file(&oversized_path).ok();
}

#[test]
fn malformed_id27_with_valid_container_bounds_still_fails_load() {
    let malformed_path = write_cell_visibility_load_fixture(
        Some(prl_format::SectionBlob {
            section_id: SectionId::DeltaShVolumes as u32,
            version: 1,
            data: vec![0],
        }),
        "postretro_test_delta_sh_malformed.prl",
    );
    assert!(
        load_prl(malformed_path.to_str().unwrap()).is_err(),
        "id-27 malformed bytes retain the established hard-fail path"
    );
    std::fs::remove_file(malformed_path).ok();
}

#[test]
fn out_of_bounds_id45_above_binding_floor_still_fails_load() {
    let path = write_cell_visibility_load_fixture(
        Some(prl_format::SectionBlob {
            section_id: SectionId::AnimatedDirectShDeltaVolumes as u32,
            version: 1,
            data: vec![0],
        }),
        "postretro_test_animated_direct_sh_over_binding_floor.prl",
    );
    mark_section_out_of_bounds_above_binding_floor(&path, SectionId::AnimatedDirectShDeltaVolumes);

    let err = load_prl(path.to_str().unwrap()).unwrap_err();
    assert!(matches!(
        err,
        PrlLoadError::FormatError(prl_format::FormatError::SectionOutOfBounds { .. })
    ));
    std::fs::remove_file(path).ok();
}

#[test]
fn out_of_bounds_id41_above_binding_floor_still_fails_load() {
    let path = write_prl_load_fixture(
        vec![
            static_alpha_lights_blob(),
            prl_format::SectionBlob {
                section_id: SectionId::DirectShVolume as u32,
                version: 1,
                data: DirectShVolumeSection::placeholder().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::EntityShadowLights as u32,
                version: 1,
                data: EntityShadowLightsSection {
                    light_indices: vec![0],
                }
                .to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::DirectShDeltaVolumes as u32,
                version: 1,
                data: vec![0],
            },
        ],
        "postretro_test_direct_sh_over_binding_floor.prl",
    );
    mark_section_out_of_bounds_above_binding_floor(&path, SectionId::DirectShDeltaVolumes);

    let err = load_prl(path.to_str().unwrap()).unwrap_err();
    assert!(matches!(
        err,
        PrlLoadError::FormatError(prl_format::FormatError::SectionOutOfBounds { .. })
    ));
    std::fs::remove_file(path).ok();
}

#[test]
fn load_prl_lowers_cell_visibility_section_into_coupling() {
    let path = write_cell_visibility_load_fixture(
        Some(prl_format::SectionBlob {
            section_id: SectionId::CellVisibility as u32,
            version: 1,
            data: CellVisibilitySection {
                cell_count: 2,
                component_ids: vec![0, 0],
                coupled_pairs: vec![CoupledPairRecord {
                    cell_a: 0,
                    cell_b: 1,
                    distance: 128,
                    aperture: 64,
                }],
            }
            .to_bytes()
            .unwrap(),
        }),
        "postretro_test_cell_visibility_loaded.prl",
    );

    let world = load_prl(path.to_str().unwrap()).expect("valid CellVisibility must load");

    assert_eq!(
        world.coupling(0, 1),
        crate::prl::CouplingTuple {
            perceivable: true,
            distance: Some(128),
            aperture: Some(64),
        }
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn load_prl_missing_cell_visibility_uses_conservative_coupling_fallback() {
    let path =
        write_cell_visibility_load_fixture(None, "postretro_test_cell_visibility_absent.prl");

    let world = load_prl(path.to_str().unwrap()).expect("missing optional section must load");

    assert_eq!(
        world.coupling(0, 1),
        crate::prl::CouplingTuple {
            perceivable: true,
            distance: None,
            aperture: None,
        }
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn load_prl_rejects_malformed_cell_visibility_section() {
    let path = write_cell_visibility_load_fixture(
        Some(prl_format::SectionBlob {
            section_id: SectionId::CellVisibility as u32,
            version: 1,
            data: vec![0],
        }),
        "postretro_test_cell_visibility_malformed.prl",
    );

    let err = load_prl(path.to_str().unwrap()).unwrap_err();

    assert!(
        matches!(
            err,
            PrlLoadError::SectionValidation {
                section: "CellVisibility",
                ..
            }
        ),
        "got {err:?}"
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn load_prl_rejects_cell_visibility_section_larger_than_fanout_bound() {
    let max_size = CellVisibilitySection::max_encoded_len(2).unwrap() as usize;
    let path = write_cell_visibility_load_fixture(
        Some(prl_format::SectionBlob {
            section_id: SectionId::CellVisibility as u32,
            version: 1,
            data: vec![0; max_size + 1],
        }),
        "postretro_test_cell_visibility_oversized.prl",
    );

    let err = load_prl(path.to_str().unwrap()).unwrap_err();
    assert!(
        matches!(
            err,
            PrlLoadError::SectionValidation {
                section: "CellVisibility",
                ..
            }
        ),
        "got {err:?}"
    );
    std::fs::remove_file(path).ok();
}

fn matching_direct_and_base_sh() -> (DirectShVolumeSection, OctahedralShVolumeSection) {
    let mut direct = DirectShVolumeSection::placeholder();
    let mut base = OctahedralShVolumeSection::placeholder();

    direct.grid_origin = [1.0, 2.0, 3.0];
    base.grid_origin = direct.grid_origin;
    direct.cell_size = [0.5, 0.75, 1.25];
    base.cell_size = direct.cell_size;
    direct.grid_dimensions = [2, 3, 4];
    base.grid_dimensions = direct.grid_dimensions;
    direct.tile_dimension = 6;
    base.tile_dimension = direct.tile_dimension;
    direct.tile_border = 1;
    base.tile_border = direct.tile_border;
    direct.atlas_dimensions = [24, 18];
    base.atlas_dimensions = direct.atlas_dimensions;
    direct.atlas_tiles_per_row = 4;
    base.atlas_tiles_per_row = direct.atlas_tiles_per_row;
    direct.layer_count = 1;
    base.layer_count = direct.layer_count;
    direct.tiles_per_layer = 24;
    base.tiles_per_layer = direct.tiles_per_layer;

    (direct, base)
}

fn assert_direct_sh_layout_message(err: PrlLoadError, expected: &str) {
    match err {
        PrlLoadError::SectionValidation { section, message } => {
            assert_eq!(section, "DirectShVolume");
            assert!(
                message.contains(expected),
                "expected validation message to contain `{expected}`, got `{message}`"
            );
        }
        other => panic!("unexpected validation error: {other:?}"),
    }
}

fn static_light() -> MapLight {
    MapLight {
        origin: [0.0, 0.0, 0.0],
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 8.0,
        cone_angle_inner: 0.0,
        cone_angle_outer: 0.0,
        cone_direction: [0.0, 0.0, 0.0],
        is_dynamic: false,
        casts_entity_shadows: false,
        animated_slot: None,
        tags: Vec::new(),
        cell_index: ALPHA_LIGHT_LEAF_UNASSIGNED,
        shadow_type: ShadowType::StaticLightMap,
    }
}

fn dynamic_light() -> MapLight {
    MapLight {
        is_dynamic: true,
        ..static_light()
    }
}

fn sample_kinematic_vertex(position: [f32; 3]) -> PrlVertex {
    PrlVertex::new(
        position,
        [0.25, 0.5],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    )
}

fn sample_kinematic_section() -> KinematicGeometrySection {
    KinematicGeometrySection {
        version: KINEMATIC_GEOMETRY_VERSION,
        movers: vec![KinematicMoverRecord {
            mover_id: 7,
            name: "lift".to_string(),
            tags: vec!["platform".to_string()],
            origin: [1.0, 2.0, 3.0],
            path: "a".to_string(),
            speed: 2.0,
            wait_ms: 125.0,
            move_mode: 1,
            start_on_spawn: true,
            vertices: vec![
                sample_kinematic_vertex([0.0, 0.0, 0.0]),
                sample_kinematic_vertex([1.0, 0.0, 0.0]),
                sample_kinematic_vertex([0.0, 1.0, 0.0]),
            ],
            indices: vec![0, 1, 2],
            face_meta: vec![PrlFaceMeta {
                leaf_index: 0,
                texture_index: 0,
            }],
            spin_axis: [0.0; 3],
            spin_speed_deg_s: 0.0,
            spin_accel_deg_s2: 0.0,
            carry_yaw: false,
            block_policy: "displace".to_string(),
            crush_damage: 0.0,
            crush_interval_ms: 0.0,
            auto_close_ms: None,
            open_event: None,
            close_event: None,
            blocked_event: None,
            crush_event: None,
            sealed_portal_ids: Vec::new(),
            carried_lights: Vec::new(),
            open_sound: None,
            close_sound: None,
            blocked_sound: None,
            crush_sound: None,
        }],
        waypoints: vec![
            KinematicWaypointRecord {
                name: "a".to_string(),
                next: "b".to_string(),
                origin: [1.0, 2.0, 3.0],
            },
            KinematicWaypointRecord {
                name: "b".to_string(),
                next: String::new(),
                origin: [3.0, 2.0, 3.0],
            },
        ],
    }
}

fn assert_kinematic_validation_message(err: PrlLoadError, expected: &str) {
    match err {
        PrlLoadError::SectionValidation { section, message } => {
            assert_eq!(section, "KinematicGeometry");
            assert!(
                message.contains(expected),
                "expected validation message to contain `{expected}`, got `{message}`"
            );
        }
        other => panic!("unexpected validation error: {other:?}"),
    }
}

#[test]
fn kinematic_geometry_rejects_duplicate_mover_ids() {
    let mut section = sample_kinematic_section();
    section.movers.push(section.movers[0].clone());

    let err = convert_kinematic_geometry_section(section).unwrap_err();

    assert_kinematic_validation_message(err, "duplicate mover_id");
}

#[test]
fn kinematic_geometry_v4_loads_with_no_sealed_portals() {
    let mut section = sample_kinematic_section();
    section.version = KINEMATIC_GEOMETRY_VERSION_V4;

    let geometry = convert_kinematic_geometry_section(
        KinematicGeometrySection::from_bytes(&section.to_bytes())
            .expect("v4 section bytes must remain readable"),
    )
    .expect("v4 section must remain runtime-loadable");

    assert!(geometry.movers[0].sealed_portal_ids.is_empty());
    assert!(geometry.movers[0].carried_lights.is_empty());
}

#[test]
fn kinematic_geometry_v5_loads_sealed_portals_with_no_member_lights() {
    let mut section = sample_kinematic_section();
    section.version = KINEMATIC_GEOMETRY_VERSION_V5;
    section.movers[0].sealed_portal_ids = vec![1];

    let geometry = convert_kinematic_geometry_section(
        KinematicGeometrySection::from_bytes(&section.to_bytes())
            .expect("v5 section bytes must remain readable"),
    )
    .expect("v5 section must remain runtime-loadable");

    assert_eq!(geometry.movers[0].sealed_portal_ids, vec![1]);
    assert!(geometry.movers[0].carried_lights.is_empty());
}

#[test]
fn kinematic_geometry_v7_loads_mover_sounds_and_v6_loads_none() {
    let mut section = sample_kinematic_section();
    section.movers[0].open_sound = Some("sfx/door_open".to_string());
    section.movers[0].blocked_sound = Some("sfx/door_blocked".to_string());

    let v7 = convert_kinematic_geometry_section(
        KinematicGeometrySection::from_bytes(&section.to_bytes())
            .expect("v7 section bytes must be readable"),
    )
    .expect("v7 section must be runtime-loadable");
    let mover = &v7.movers[0];
    assert_eq!(mover.open_sound.as_deref(), Some("sfx/door_open"));
    assert_eq!(mover.close_sound, None);
    assert_eq!(mover.blocked_sound.as_deref(), Some("sfx/door_blocked"));
    assert_eq!(mover.crush_sound, None);

    section.version = KINEMATIC_GEOMETRY_VERSION_V6;
    let v6 = convert_kinematic_geometry_section(
        KinematicGeometrySection::from_bytes(&section.to_bytes())
            .expect("v6 section bytes must remain readable"),
    )
    .expect("v6 section must remain runtime-loadable");
    let mover = &v6.movers[0];
    assert_eq!(mover.open_sound, None);
    assert_eq!(mover.close_sound, None);
    assert_eq!(mover.blocked_sound, None);
    assert_eq!(mover.crush_sound, None);
}

#[test]
fn kinematic_geometry_drops_sealed_portals_outside_loaded_portal_array() {
    let mut geometry = convert_kinematic_geometry_section(sample_kinematic_section())
        .expect("sample section must lower");
    geometry.movers[0].sealed_portal_ids = vec![0, 2, u32::MAX];

    let dropped = drop_out_of_range_sealed_portal_ids(&mut geometry, 2);

    assert_eq!(dropped, 2);
    assert_eq!(geometry.movers[0].sealed_portal_ids, vec![0]);
}

#[test]
fn kinematic_geometry_keeps_unique_dynamic_carried_light_link() {
    let mut section = sample_kinematic_section();
    section.movers[0].carried_lights = vec![MemberLight {
        alpha_light_index: 0,
        local_offset: [1.0, 2.0, 3.0],
    }];
    let mut geometry = convert_kinematic_geometry_section(section).unwrap();

    let dropped = drop_invalid_carried_light_links(&mut geometry, &[dynamic_light()]);

    assert_eq!(dropped, 0);
    assert_eq!(geometry.movers[0].carried_lights.len(), 1);
    assert_eq!(
        geometry.movers[0].carried_lights[0].local_offset,
        Vec3::new(1.0, 2.0, 3.0)
    );
}

// Regression: finite V6 mover and offset components could overflow when
// composed and then propagate infinity to GPU-facing light buffers.
#[test]
fn kinematic_geometry_drops_carried_light_with_unrepresentable_authored_position() {
    let mut section = sample_kinematic_section();
    section.movers[0].origin = [3.0e38, 0.0, 0.0];
    section.movers[0].carried_lights = vec![MemberLight {
        alpha_light_index: 0,
        local_offset: [3.0e38, 0.0, 0.0],
    }];
    section.waypoints[0].origin = [3.0e38, 0.0, 0.0];
    section.waypoints[1].origin = [3.0e38, 1.0, 0.0];
    let decoded = KinematicGeometrySection::from_bytes(&section.to_bytes())
        .expect("finite wire components should decode");
    let mut geometry = convert_kinematic_geometry_section(decoded).unwrap();
    let capture = LogCapture::start();

    let dropped = drop_invalid_carried_light_links(&mut geometry, &[dynamic_light()]);

    assert_eq!(dropped, 1);
    assert!(geometry.movers[0].carried_lights.is_empty());
    capture.assert_logged_once(
        Level::Warn,
        "because its authored position is outside the runtime f32 range",
    );
}

#[test]
fn kinematic_geometry_drops_carried_light_index_outside_alpha_lights() {
    let mut section = sample_kinematic_section();
    section.movers[0].carried_lights = vec![MemberLight {
        alpha_light_index: 1,
        local_offset: [1.0, 2.0, 3.0],
    }];
    let mut geometry = convert_kinematic_geometry_section(section).unwrap();

    let dropped = drop_invalid_carried_light_links(&mut geometry, &[dynamic_light()]);

    assert_eq!(dropped, 1);
    assert!(geometry.movers[0].carried_lights.is_empty());
}

#[test]
fn kinematic_geometry_drops_carried_link_to_baked_alpha_light() {
    let mut section = sample_kinematic_section();
    section.movers[0].carried_lights = vec![MemberLight {
        alpha_light_index: 0,
        local_offset: [1.0, 2.0, 3.0],
    }];
    let mut geometry = convert_kinematic_geometry_section(section).unwrap();

    let dropped = drop_invalid_carried_light_links(&mut geometry, &[static_light()]);

    assert_eq!(dropped, 1);
    assert!(geometry.movers[0].carried_lights.is_empty());
}

#[test]
fn kinematic_geometry_drops_every_duplicate_carried_light_reference() {
    let mut section = sample_kinematic_section();
    section.movers[0].carried_lights = vec![MemberLight {
        alpha_light_index: 0,
        local_offset: [1.0, 2.0, 3.0],
    }];
    let mut later_mover = section.movers[0].clone();
    later_mover.mover_id = 8;
    later_mover.name = "door".to_string();
    later_mover.carried_lights = vec![MemberLight {
        alpha_light_index: 0,
        local_offset: [4.0, 5.0, 6.0],
    }];
    section.movers.push(later_mover);
    let mut geometry = convert_kinematic_geometry_section(section).unwrap();

    let dropped = drop_invalid_carried_light_links(&mut geometry, &[dynamic_light()]);

    assert_eq!(dropped, 2);
    assert!(
        geometry
            .movers
            .iter()
            .all(|mover| mover.carried_lights.is_empty())
    );
}

#[test]
fn kinematic_geometry_rejects_empty_mover_geometry() {
    let mut section = sample_kinematic_section();
    section.movers[0].vertices.clear();
    section.movers[0].indices.clear();

    let err = convert_kinematic_geometry_section(section).unwrap_err();

    assert_kinematic_validation_message(err, "geometry must contain vertices and indices");
}

#[test]
fn kinematic_geometry_rejects_zero_length_waypoint_segments() {
    let mut section = sample_kinematic_section();
    section.waypoints[1].origin = section.waypoints[0].origin;

    let err = convert_kinematic_geometry_section(section).unwrap_err();

    assert_kinematic_validation_message(err, "zero-length segment");
}

#[test]
fn kinematic_geometry_rejects_near_zero_waypoint_segments() {
    let mut section = sample_kinematic_section();
    section.waypoints[1].origin = [
        section.waypoints[0].origin[0] + KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH * 0.5,
        section.waypoints[0].origin[1],
        section.waypoints[0].origin[2],
    ];

    let err = convert_kinematic_geometry_section(section).unwrap_err();

    assert_kinematic_validation_message(err, "zero-length segment");
}

#[test]
fn kinematic_geometry_accepts_single_waypoint_pure_rotator() {
    let mut section = sample_kinematic_section();
    section.movers[0].spin_axis = [0.0, 1.0, 0.0];
    section.movers[0].spin_speed_deg_s = 90.0;
    section.waypoints.truncate(1);
    section.waypoints[0].next.clear();

    let geometry = convert_kinematic_geometry_section(section)
        .expect("a rotating mover can use a single waypoint");

    assert_eq!(geometry.movers.len(), 1);
    assert_eq!(geometry.waypoints.len(), 1);
}

#[test]
fn kinematic_geometry_rejects_single_waypoint_nonzero_spin_with_zero_axis() {
    for spin_axis in [[0.0; 3], [f32::MIN_POSITIVE; 3]] {
        let mut section = sample_kinematic_section();
        section.movers[0].spin_axis = spin_axis;
        section.movers[0].spin_speed_deg_s = 90.0;
        section.waypoints.truncate(1);
        section.waypoints[0].next.clear();

        let err = convert_kinematic_geometry_section(section).unwrap_err();

        assert_kinematic_validation_message(err, "spin_axis normalizes to zero");
    }
}

// Regression: non-zero PRL degrees could authorize a pure rotator that is static in radians.
#[test]
fn kinematic_geometry_rejects_nonzero_spin_speed_that_underflows_in_radians() {
    let mut section = sample_kinematic_section();
    section.movers[0].spin_axis = [0.0, 1.0, 0.0];
    section.movers[0].spin_speed_deg_s = f32::from_bits(1);
    section.waypoints.truncate(1);
    section.waypoints[0].next.clear();

    let err = convert_kinematic_geometry_section(section).unwrap_err();

    assert_kinematic_validation_message(err, "conversion to radians/sec");
}

// Regression: positive PRL acceleration could become zero radians and turn a ramp into a snap.
#[test]
fn kinematic_geometry_rejects_positive_spin_accel_that_underflows_in_radians() {
    let mut section = sample_kinematic_section();
    section.movers[0].spin_accel_deg_s2 = f32::from_bits(1);

    let err = convert_kinematic_geometry_section(section).unwrap_err();

    assert_kinematic_validation_message(err, "conversion to radians/sec²");
}

#[test]
fn kinematic_geometry_rejects_single_waypoint_without_spin() {
    let mut section = sample_kinematic_section();
    section.waypoints.truncate(1);
    section.waypoints[0].next.clear();

    let err = convert_kinematic_geometry_section(section).unwrap_err();

    assert_kinematic_validation_message(err, "at least 2 required");
}

#[test]
fn kinematic_geometry_rejects_mover_origin_that_differs_from_first_waypoint() {
    let mut section = sample_kinematic_section();
    section.movers[0].origin[0] += KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH * 2.0;

    let err = convert_kinematic_geometry_section(section).unwrap_err();

    assert_kinematic_validation_message(err, "must match first waypoint");
}

#[test]
fn direct_sh_layout_rejects_grid_origin_mismatch() {
    let (mut direct, base) = matching_direct_and_base_sh();
    direct.grid_origin[0] += 1.0;

    let err = validate_direct_sh_layout(&direct, &base).unwrap_err();

    assert_direct_sh_layout_message(err, "grid_origin");
}

#[test]
fn direct_sh_layout_rejects_cell_size_mismatch() {
    let (mut direct, base) = matching_direct_and_base_sh();
    direct.cell_size[2] += 0.25;

    let err = validate_direct_sh_layout(&direct, &base).unwrap_err();

    assert_direct_sh_layout_message(err, "cell_size");
}

#[test]
fn direct_sh_layout_requires_matching_irradiance_format() {
    let (mut direct, base) = matching_direct_and_base_sh();

    validate_direct_sh_layout(&direct, &base)
        .expect("matching id-35 and id-34 stored geometry and format must be accepted");

    direct.irradiance_format = IRRADIANCE_FORMAT_RGBA16F;
    let err = validate_direct_sh_layout(&direct, &base).unwrap_err();

    assert_direct_sh_layout_message(err, "irradiance_format");
}

#[test]
fn entity_shadow_selection_rejects_animated_static_light_for_degrade_path() {
    let mut light = static_light();
    light.animated_slot = Some(0);

    let err = validate_entity_shadow_light_selection(&[0], &[light]).unwrap_err();

    match err {
        PrlLoadError::SectionValidation { section, message } => {
            assert_eq!(section, "EntityShadowLights");
            assert!(
                message.contains("animated static light"),
                "unexpected validation message: {message}"
            );
        }
        other => panic!("unexpected validation error: {other:?}"),
    }
}
