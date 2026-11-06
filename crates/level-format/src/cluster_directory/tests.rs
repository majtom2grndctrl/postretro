use super::*;
use crate::{
    animated_direct_sh_delta_volumes::ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION,
    bvh::BvhLeaf,
    cells::{CELL_FLAG_DRAWABLE, CellRecord},
    delta_sh_volumes::AFFINITY_FACTOR,
    portals::PortalRecord,
    sh_volume::{OCTAHEDRAL_PROBE_STRIDE, OctahedralShProbe},
};

fn empty_directory() -> ClusterDirectorySection {
    ClusterDirectorySection {
        runtime_cell_count: 0,
        primitive_limit: 64,
        cell_limit: 16,
        clusters: Vec::new(),
        resources: Vec::new(),
        members: Vec::new(),
        ranges: Vec::new(),
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
        mandatory_bound: Vec::new(),
    }
}

fn one_cell_directory() -> ClusterDirectorySection {
    ClusterDirectorySection {
        runtime_cell_count: 1,
        primitive_limit: 64,
        cell_limit: 16,
        clusters: vec![ClusterRecord {
            bounds_min: [0.0, 0.0, 0.0],
            bounds_max: [1.0, 1.0, 1.0],
            member_start: 0,
            member_count: 1,
            range_start: 0,
            range_count: 0,
            primitive_count: 0,
            flags: 0,
        }],
        resources: Vec::new(),
        members: vec![0],
        ranges: Vec::new(),
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
        mandatory_bound: Vec::new(),
    }
}

fn cells(count: usize) -> CellsSection {
    CellsSection {
        cells: (0..count)
            .map(|index| CellRecord {
                bounds_min: [index as f32, 0.0, 0.0],
                bounds_max: [index as f32 + 1.0, 1.0, 1.0],
                flags: CELL_FLAG_DRAWABLE,
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
            })
            .collect(),
        portal_refs: Vec::new(),
    }
}

fn empty_bvh() -> BvhSection {
    BvhSection {
        nodes: Vec::new(),
        leaves: Vec::new(),
        root_node_index: 0,
    }
}

fn base_volume(dimensions: [u32; 3], probes: Vec<OctahedralShProbe>) -> OctahedralShVolumeSection {
    OctahedralShVolumeSection {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: dimensions,
        probe_stride: OCTAHEDRAL_PROBE_STRIDE,
        tile_dimension: 6,
        tile_border: 1,
        atlas_dimensions: [0, 0],
        layer_count: 0,
        tiles_per_layer: 0,
        atlas_tiles_per_row: 0,
        probes,
        irradiance_format: 1,
        compact_atlas: Vec::new(),
        animation_descriptors: Vec::new(),
        slot_for_map_light: Vec::new(),
    }
}

#[test]
fn cluster_directory_empty_and_single_cluster_round_trip_exact_wire() {
    let empty = empty_directory();
    let bytes = empty.try_to_bytes().unwrap();
    assert_eq!(bytes.len(), HEADER_SIZE + 4);
    assert_eq!(ClusterDirectorySection::from_bytes(&bytes).unwrap(), empty);

    let single = one_cell_directory();
    let bytes = single.try_to_bytes().unwrap();
    assert_eq!(
        bytes.len(),
        HEADER_SIZE + CLUSTER_RECORD_SIZE + MEMBER_RECORD_SIZE + 4
    );
    assert_eq!(ClusterDirectorySection::from_bytes(&bytes).unwrap(), single);
}

#[test]
fn cluster_directory_parser_rejects_old_version_counts_length_and_unknown_values() {
    let bytes = empty_directory().try_to_bytes().unwrap();
    let mut bad = bytes.clone();
    bad[0..4].copy_from_slice(&2u32.to_le_bytes());
    assert!(matches!(
        ClusterDirectorySection::from_bytes(&bad),
        Err(ClusterDirectoryError::VersionMismatch { .. })
    ));
    // A count the bytes cannot hold fails the header-derived prefix check.
    let mut bad = bytes.clone();
    bad[32..36].copy_from_slice(&2u32.to_le_bytes());
    assert!(matches!(
        ClusterDirectorySection::from_bytes(&bad),
        Err(ClusterDirectoryError::InvalidData(_))
    ));
    let mut bad = bytes;
    bad.push(0);
    assert!(matches!(
        ClusterDirectorySection::from_bytes(&bad),
        Err(ClusterDirectoryError::BoundTrailingBytes { extra: 1 })
    ));

    let mut section = one_cell_directory();
    section.resources.push(ClusterResourceRecord {
        section_id: 34,
        domain: ClusterResourceDomain::DenseProbe,
        dimensions: [1, 1, 1],
    });
    section.clusters[0].range_count = 1;
    section.ranges.push(ClusterRangeRecord {
        resource_index: 0,
        start: 0,
        count: 1,
    });
    let mut bytes = section.try_to_bytes().unwrap();
    let resource_offset = HEADER_SIZE + CLUSTER_RECORD_SIZE;
    bytes[resource_offset + 4..resource_offset + 8].copy_from_slice(&9u32.to_le_bytes());
    assert!(matches!(
        ClusterDirectorySection::from_bytes(&bytes),
        Err(ClusterDirectoryError::InvalidData(_))
    ));
}

#[test]
fn cluster_directory_appends_seams_and_cluster_hints_after_legacy_tables() {
    let mut section = one_cell_directory();
    section.clusters[0].primitive_count = 65;
    section.clusters[0].flags = CLUSTER_FLAG_INDIVISIBLE_OVERSIZE;
    section.seam_portal_ids = vec![1, 2];
    section.cluster_hints = vec![ClusterHintRecord {
        cluster_id: 0,
        flags: CLUSTER_HINT_FLAG_PINNED,
        priority: 3,
    }];
    let bytes = section.try_to_bytes().unwrap();
    assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 3);
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 1);
    assert_eq!(u32::from_le_bytes(bytes[32..36].try_into().unwrap()), 2);
    assert_eq!(u32::from_le_bytes(bytes[36..40].try_into().unwrap()), 1);
    assert_eq!(
        bytes.len(),
        HEADER_SIZE
            + CLUSTER_RECORD_SIZE
            + MEMBER_RECORD_SIZE
            + 2 * MEMBER_RECORD_SIZE
            + CLUSTER_HINT_RECORD_SIZE
            + 4
    );
    assert_eq!(
        ClusterDirectorySection::from_bytes(&bytes).unwrap(),
        section
    );
    assert_eq!(
        ClusterDirectorySection::from_bytes(&bytes)
            .unwrap()
            .clusters[0]
            .flags,
        CLUSTER_FLAG_INDIVISIBLE_OVERSIZE
    );

    let seam_start = HEADER_SIZE + CLUSTER_RECORD_SIZE + MEMBER_RECORD_SIZE;
    let mut malformed_order = bytes.clone();
    malformed_order[seam_start + MEMBER_RECORD_SIZE..seam_start + 2 * MEMBER_RECORD_SIZE]
        .copy_from_slice(&1u32.to_le_bytes());
    assert!(matches!(
        ClusterDirectorySection::from_bytes(&malformed_order),
        Err(ClusterDirectoryError::SeamPortalOrder)
    ));

    let mut malformed = bytes;
    // Past the hint record's reserved word sits the four-byte bound count.
    let hint_reserved_offset = malformed.len() - 8;
    malformed[hint_reserved_offset..hint_reserved_offset + 4].copy_from_slice(&1u32.to_le_bytes());
    assert!(matches!(
        ClusterDirectorySection::from_bytes(&malformed),
        Err(ClusterDirectoryError::ClusterHintReserved { .. })
    ));
}

#[test]
fn cluster_directory_rejects_noncanonical_hint_records() {
    let mut section = one_cell_directory();
    section.seam_portal_ids = vec![1, 1];
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::SeamPortalOrder)
    ));

    let mut section = one_cell_directory();
    section.cluster_hints = vec![ClusterHintRecord {
        cluster_id: 0,
        flags: 0,
        priority: 0,
    }];
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::ClusterHintNoOp { .. })
    ));

    let mut section = one_cell_directory();
    section.cluster_hints = vec![ClusterHintRecord {
        cluster_id: 0,
        flags: 2,
        priority: 0,
    }];
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::ClusterHintFlagsInvalid { .. })
    ));

    let mut section = one_cell_directory();
    section.cluster_hints = vec![ClusterHintRecord {
        cluster_id: 0,
        flags: 0,
        priority: 4,
    }];
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::ClusterHintPriorityOutOfRange { .. })
    ));

    let mut section = one_cell_directory();
    section.cluster_hints = vec![ClusterHintRecord {
        cluster_id: 1,
        flags: CLUSTER_HINT_FLAG_PINNED,
        priority: 0,
    }];
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::ClusterHintClusterOutOfRange { .. })
    ));
}

#[test]
fn cluster_directory_structure_rejects_membership_bounds_flags_order_and_grid_ranges() {
    let mut section = one_cell_directory();
    section.members[0] = 1;
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::CellOutOfRange { .. })
    ));

    let mut section = one_cell_directory();
    section.clusters[0].bounds_min[0] = f32::NAN;
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::InvalidData(_))
    ));
    let mut section = one_cell_directory();
    section.clusters[0].bounds_min[0] = -0.0;
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::InvalidData(_))
    ));
    let mut section = one_cell_directory();
    section.clusters[0].flags = 2;
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::InvalidData(_))
    ));

    let mut section = one_cell_directory();
    section.resources.push(ClusterResourceRecord {
        section_id: 34,
        domain: ClusterResourceDomain::DenseProbe,
        dimensions: [2, 1, 1],
    });
    section.clusters[0].range_count = 1;
    section.ranges.push(ClusterRangeRecord {
        resource_index: 0,
        start: u32::MAX,
        count: 2,
    });
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::SizeOverflow(_))
    ));

    let mut section = one_cell_directory();
    section.resources.push(ClusterResourceRecord {
        section_id: 34,
        domain: ClusterResourceDomain::DenseProbe,
        dimensions: [2, 1, 1],
    });
    section.clusters[0].range_count = 1;
    section.ranges.push(ClusterRangeRecord {
        resource_index: 0,
        start: 1,
        count: 2,
    });
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::GridRangeOutOfRange { .. })
    ));
}

#[test]
fn cluster_directory_structure_rejects_bad_range_order_overlap_and_coalescing() {
    let mut section = one_cell_directory();
    section.resources.push(ClusterResourceRecord {
        section_id: 27,
        domain: ClusterResourceDomain::AffinityCell,
        dimensions: [4, 1, 1],
    });
    section.clusters[0].range_count = 2;
    section.ranges = vec![
        ClusterRangeRecord {
            resource_index: 0,
            start: 0,
            count: 1,
        },
        ClusterRangeRecord {
            resource_index: 0,
            start: 1,
            count: 1,
        },
    ];
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::InvalidData(_))
    ));
    section.ranges[1].start = 0;
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::InvalidData(_))
    ));
    section.ranges.truncate(1);
    section.clusters[0].range_count = 1;
    section.validate_structure().unwrap();
}

#[test]
fn cluster_directory_structure_accepts_an_affinity_cell_two_clusters_share() {
    let section = ClusterDirectorySection {
        runtime_cell_count: 2,
        primitive_limit: 4,
        cell_limit: 1,
        clusters: vec![
            ClusterRecord {
                bounds_min: [0.0; 3],
                bounds_max: [1.0; 3],
                member_start: 0,
                member_count: 1,
                range_start: 0,
                range_count: 1,
                primitive_count: 0,
                flags: 0,
            },
            ClusterRecord {
                bounds_min: [1.0, 0.0, 0.0],
                bounds_max: [2.0, 1.0, 1.0],
                member_start: 1,
                member_count: 1,
                range_start: 1,
                range_count: 1,
                primitive_count: 0,
                flags: 0,
            },
        ],
        resources: vec![ClusterResourceRecord {
            section_id: 27,
            domain: ClusterResourceDomain::AffinityCell,
            dimensions: [1, 1, 1],
        }],
        members: vec![0, 1],
        ranges: vec![
            ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 1,
            },
            ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 1,
            },
        ],
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
        mandatory_bound: Vec::new(),
    };
    let bytes = section.try_to_bytes().unwrap();
    assert_eq!(
        ClusterDirectorySection::from_bytes(&bytes).unwrap(),
        section
    );
}

#[test]
fn cluster_directory_structure_rejects_membership_gap_and_duplicate() {
    let mut section = ClusterDirectorySection {
        runtime_cell_count: 2,
        primitive_limit: 4,
        cell_limit: 2,
        clusters: vec![ClusterRecord {
            bounds_min: [0.0; 3],
            bounds_max: [1.0; 3],
            member_start: 0,
            member_count: 2,
            range_start: 0,
            range_count: 0,
            primitive_count: 0,
            flags: 0,
        }],
        resources: Vec::new(),
        members: vec![0, 0],
        ranges: Vec::new(),
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
        mandatory_bound: Vec::new(),
    };
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::InvalidData(_))
    ));
    section.members = vec![0];
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::InvalidData(_))
    ));
}

#[test]
fn cluster_directory_structure_rejects_budget_dimension_and_resource_order_violations() {
    let mut section = one_cell_directory();
    section.clusters[0].primitive_count = section.primitive_limit + 1;
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::InvalidData(_))
    ));
    section.clusters[0].flags = CLUSTER_FLAG_INDIVISIBLE_OVERSIZE;
    section.validate_structure().unwrap();

    section.resources = vec![ClusterResourceRecord {
        section_id: 34,
        domain: ClusterResourceDomain::DenseProbe,
        dimensions: [1, 0, 1],
    }];
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::InvalidData(_))
    ));
    section.resources = vec![
        ClusterResourceRecord {
            section_id: 35,
            domain: ClusterResourceDomain::DenseProbe,
            dimensions: [0, 0, 0],
        },
        ClusterResourceRecord {
            section_id: 34,
            domain: ClusterResourceDomain::DenseProbe,
            dimensions: [0, 0, 0],
        },
    ];
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::InvalidData(_))
    ));
}

#[test]
fn cluster_directory_semantics_validate_partition_connectivity_bounds_and_bvh_counts() {
    let cells = cells(2);
    let portals = PortalsSection {
        vertices: Vec::new(),
        portals: vec![PortalRecord {
            vertex_start: 0,
            vertex_count: 0,
            front_leaf: 0,
            back_leaf: 1,
        }],
    };
    let locator = CellLocatorSection {
        root: CellLocatorChild::Cell(0),
        nodes: Vec::new(),
    };
    let mut directory = ClusterDirectorySection {
        runtime_cell_count: 2,
        primitive_limit: 4,
        cell_limit: 4,
        clusters: vec![ClusterRecord {
            bounds_min: [0.0, 0.0, 0.0],
            bounds_max: [2.0, 1.0, 1.0],
            member_start: 0,
            member_count: 2,
            range_start: 0,
            range_count: 0,
            primitive_count: 1,
            flags: 0,
        }],
        resources: Vec::new(),
        members: vec![0, 1],
        ranges: Vec::new(),
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
        mandatory_bound: Vec::new(),
    };
    let mut bvh = empty_bvh();
    bvh.leaves.push(BvhLeaf {
        aabb_min: [0.0; 3],
        material_bucket_id: 0,
        aabb_max: [1.0; 3],
        index_offset: 0,
        index_count: 3,
        cell_id: 1,
        chunk_range_start: 0,
        chunk_range_count: 0,
    });
    let inputs = ClusterDirectoryValidationInputs {
        cells: &cells,
        portals: &portals,
        bvh: &bvh,
        cell_locator: &locator,
        sh: ClusterDirectoryShInventory::default(),
    };
    directory.validate_semantics(inputs).unwrap();

    let disconnected = PortalsSection {
        vertices: Vec::new(),
        portals: Vec::new(),
    };
    let inputs = ClusterDirectoryValidationInputs {
        portals: &disconnected,
        ..inputs
    };
    assert!(matches!(
        directory.validate_semantics(inputs),
        Err(ClusterDirectoryError::CanonicalPartitionMismatch(_))
    ));
    directory.clusters[0].bounds_max[0] = 3.0;
    let inputs = ClusterDirectoryValidationInputs {
        portals: &portals,
        ..inputs
    };
    assert!(matches!(
        directory.validate_semantics(inputs),
        Err(ClusterDirectoryError::CanonicalPartitionMismatch(_))
    ));
}

#[test]
fn cluster_directory_semantics_rejects_connected_noncanonical_greedy_membership() {
    let cells = cells(3);
    let portals = PortalsSection {
        vertices: Vec::new(),
        portals: vec![
            PortalRecord {
                vertex_start: 0,
                vertex_count: 0,
                front_leaf: 0,
                back_leaf: 1,
            },
            PortalRecord {
                vertex_start: 0,
                vertex_count: 0,
                front_leaf: 1,
                back_leaf: 2,
            },
        ],
    };
    let bvh = BvhSection {
        nodes: Vec::new(),
        leaves: (0..3)
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
    let locator = CellLocatorSection {
        root: CellLocatorChild::Cell(0),
        nodes: Vec::new(),
    };
    // Regression: [0], [1, 2] is connected and within both limits, but the
    // pinned greedy rule emits [0, 1], [2].
    let directory = ClusterDirectorySection {
        runtime_cell_count: 3,
        primitive_limit: 2,
        cell_limit: 3,
        clusters: vec![
            ClusterRecord {
                bounds_min: [0.0, 0.0, 0.0],
                bounds_max: [1.0, 1.0, 1.0],
                member_start: 0,
                member_count: 1,
                range_start: 0,
                range_count: 0,
                primitive_count: 1,
                flags: 0,
            },
            ClusterRecord {
                bounds_min: [1.0, 0.0, 0.0],
                bounds_max: [3.0, 1.0, 1.0],
                member_start: 1,
                member_count: 2,
                range_start: 0,
                range_count: 0,
                primitive_count: 2,
                flags: 0,
            },
        ],
        resources: Vec::new(),
        members: vec![0, 1, 2],
        ranges: Vec::new(),
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
        mandatory_bound: Vec::new(),
    };
    directory.validate_structure().unwrap();

    let error = directory
        .validate_semantics(ClusterDirectoryValidationInputs {
            cells: &cells,
            portals: &portals,
            bvh: &bvh,
            cell_locator: &locator,
            sh: ClusterDirectoryShInventory::default(),
        })
        .unwrap_err();
    assert!(matches!(
        error,
        ClusterDirectoryError::CanonicalPartitionMismatch(_)
    ));
    assert!(error.to_string().contains("canonical greedy partition"));
}

#[test]
fn canonical_partition_cuts_a_seam_even_when_an_alternate_route_connects_its_cells() {
    let cells = cells(3);
    let portals = PortalsSection {
        vertices: Vec::new(),
        portals: vec![
            PortalRecord {
                vertex_start: 0,
                vertex_count: 0,
                front_leaf: 0,
                back_leaf: 1,
            },
            PortalRecord {
                vertex_start: 0,
                vertex_count: 0,
                front_leaf: 0,
                back_leaf: 2,
            },
            PortalRecord {
                vertex_start: 0,
                vertex_count: 0,
                front_leaf: 2,
                back_leaf: 1,
            },
        ],
    };
    let bvh = empty_bvh();
    let no_hints = canonical_cell_partition(&cells, &portals, &bvh, 8, 3, &[]).unwrap();
    assert_eq!(no_hints.members, vec![0, 1, 2]);
    assert_eq!(no_hints.clusters.len(), 1);

    let cut = canonical_cell_partition(&cells, &portals, &bvh, 8, 3, &[0]).unwrap();
    assert_eq!(cut.members, vec![0, 2, 1]);
    assert_eq!(
        cut.clusters
            .iter()
            .map(|cluster| cluster.member_count)
            .collect::<Vec<_>>(),
        vec![2, 1]
    );

    let locator = CellLocatorSection {
        root: CellLocatorChild::Cell(0),
        nodes: Vec::new(),
    };
    let cut_directory = ClusterDirectorySection {
        runtime_cell_count: 3,
        primitive_limit: 8,
        cell_limit: 3,
        clusters: cut.clusters,
        resources: Vec::new(),
        members: cut.members,
        ranges: Vec::new(),
        seam_portal_ids: vec![0],
        cluster_hints: Vec::new(),
        mandatory_bound: Vec::new(),
    };
    cut_directory
        .validate_semantics(ClusterDirectoryValidationInputs {
            cells: &cells,
            portals: &portals,
            bvh: &bvh,
            cell_locator: &locator,
            sh: ClusterDirectoryShInventory::default(),
        })
        .unwrap();

    let mut out_of_range = cut_directory.clone();
    out_of_range.seam_portal_ids = vec![3];
    let error = out_of_range
        .validate_semantics(ClusterDirectoryValidationInputs {
            cells: &cells,
            portals: &portals,
            bvh: &bvh,
            cell_locator: &locator,
            sh: ClusterDirectoryShInventory::default(),
        })
        .unwrap_err();
    assert!(matches!(
        error,
        ClusterDirectoryError::SeamPortalOutOfRange { .. }
    ));

    let mut same_cell_portals = portals.clone();
    same_cell_portals.portals[0].back_leaf = 0;
    let error = cut_directory
        .validate_semantics(ClusterDirectoryValidationInputs {
            cells: &cells,
            portals: &same_cell_portals,
            bvh: &bvh,
            cell_locator: &locator,
            sh: ClusterDirectoryShInventory::default(),
        })
        .unwrap_err();
    assert!(matches!(
        error,
        ClusterDirectoryError::SeamPortalSameCell { .. }
    ));

    let noncanonical = ClusterDirectorySection {
        clusters: no_hints.clusters,
        members: no_hints.members,
        seam_portal_ids: vec![0],
        ..cut_directory
    };
    let error = noncanonical
        .validate_semantics(ClusterDirectoryValidationInputs {
            cells: &cells,
            portals: &portals,
            bvh: &bvh,
            cell_locator: &locator,
            sh: ClusterDirectoryShInventory::default(),
        })
        .unwrap_err();
    assert!(matches!(
        error,
        ClusterDirectoryError::SeamPortalSameCluster { .. }
    ));
}

#[test]
fn cluster_directory_semantics_rejects_locator_terminal_cell_outside_runtime_count() {
    let cells = cells(1);
    let portals = PortalsSection {
        vertices: Vec::new(),
        portals: Vec::new(),
    };
    let bvh = empty_bvh();
    let locator = CellLocatorSection {
        root: CellLocatorChild::Cell(1),
        nodes: Vec::new(),
    };
    let mut probes = vec![OctahedralShProbe::default()];
    probes[0].validity = 1;
    let base = base_volume([1, 1, 1], probes);
    let mut directory = one_cell_directory();
    directory.resources = vec![ClusterResourceRecord {
        section_id: 34,
        domain: ClusterResourceDomain::DenseProbe,
        dimensions: [1, 1, 1],
    }];
    directory.clusters[0].range_count = 1;
    directory.ranges = vec![ClusterRangeRecord {
        resource_index: 0,
        start: 0,
        count: 1,
    }];

    // Regression: malformed locator terminals previously indexed cell_to_cluster and panicked.
    assert!(matches!(
        directory.validate_semantics(ClusterDirectoryValidationInputs {
            cells: &cells,
            portals: &portals,
            bvh: &bvh,
            cell_locator: &locator,
            sh: ClusterDirectoryShInventory {
                octahedral: Some(&base),
                ..Default::default()
            },
        }),
        Err(ClusterDirectoryError::InvalidData(_))
    ));
}

#[test]
fn cluster_directory_accepts_valid_empty_sparse_companion_with_nonzero_dimensions() {
    let dims = [4, 4, 4];
    let mut probes = vec![OctahedralShProbe::default(); 64];
    probes[0].validity = 1;
    let base = base_volume(dims, probes);
    let sparse = AnimatedDirectShDeltaVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: [1, 1, 1],
        tile_dimension: 6,
        tile_border: 1,
        animation_descriptor_indices: Vec::new(),
        valid_probe_masks: vec![0],
        cell_levels: vec![0],
        affinity_offsets: vec![0, 0],
        affinity_lights: Vec::new(),
        delta_subblocks: Vec::new(),
    };
    let sparse_bytes = sparse.try_to_bytes().unwrap();
    assert_eq!(sparse_bytes[0], ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION);
    let sparse = AnimatedDirectShDeltaVolumesSection::from_bytes(&sparse_bytes).unwrap();
    let cells = CellsSection {
        cells: vec![CellRecord {
            bounds_min: [-1.0; 3],
            bounds_max: [4.0; 3],
            flags: CELL_FLAG_DRAWABLE,
            face_start: 0,
            face_count: 0,
            portal_ref_start: 0,
            portal_ref_count: 0,
        }],
        portal_refs: Vec::new(),
    };
    let portals = PortalsSection {
        vertices: Vec::new(),
        portals: Vec::new(),
    };
    let bvh = empty_bvh();
    let locator = CellLocatorSection {
        root: CellLocatorChild::Cell(0),
        nodes: Vec::new(),
    };
    let mut directory = one_cell_directory();
    directory.clusters[0].bounds_min = [-1.0; 3];
    directory.clusters[0].bounds_max = [4.0; 3];
    directory.resources = vec![
        ClusterResourceRecord {
            section_id: 34,
            domain: ClusterResourceDomain::DenseProbe,
            dimensions: dims,
        },
        ClusterResourceRecord {
            section_id: 45,
            domain: ClusterResourceDomain::AffinityCell,
            dimensions: [1, 1, 1],
        },
    ];
    directory.clusters[0].range_count = 2;
    directory.ranges = vec![
        ClusterRangeRecord {
            resource_index: 0,
            start: 0,
            count: 64,
        },
        ClusterRangeRecord {
            resource_index: 1,
            start: 0,
            count: 1,
        },
    ];
    directory
        .validate_semantics(ClusterDirectoryValidationInputs {
            cells: &cells,
            portals: &portals,
            bvh: &bvh,
            cell_locator: &locator,
            sh: ClusterDirectoryShInventory {
                octahedral: Some(&base),
                animated_direct_delta: Some(&sparse),
                ..Default::default()
            },
        })
        .unwrap();
}

#[test]
fn cluster_directory_scale_zero_closure_stays_local_on_wide_grid() {
    let dims = [20, 4, 4];
    let mut probes = vec![OctahedralShProbe::default(); 320];
    probes[0].validity = 1;
    let base = base_volume(dims, probes);
    let cells = CellsSection {
        cells: vec![CellRecord {
            bounds_min: [-0.1; 3],
            bounds_max: [0.1; 3],
            flags: CELL_FLAG_DRAWABLE,
            face_start: 0,
            face_count: 0,
            portal_ref_start: 0,
            portal_ref_count: 0,
        }],
        portal_refs: Vec::new(),
    };
    let portals = PortalsSection {
        vertices: Vec::new(),
        portals: Vec::new(),
    };
    let bvh = empty_bvh();
    let locator = CellLocatorSection {
        root: CellLocatorChild::Cell(0),
        nodes: Vec::new(),
    };
    let mut directory = one_cell_directory();
    directory.clusters[0].bounds_min = [-0.1; 3];
    directory.clusters[0].bounds_max = [0.1; 3];
    directory.resources = vec![ClusterResourceRecord {
        section_id: 34,
        domain: ClusterResourceDomain::DenseProbe,
        dimensions: dims,
    }];
    for z in 0..4 {
        for y in 0..4 {
            directory.ranges.push(ClusterRangeRecord {
                resource_index: 0,
                start: y * 20 + z * 80,
                count: 4,
            });
        }
    }
    directory.clusters[0].range_count = directory.ranges.len() as u32;
    directory
        .validate_semantics(ClusterDirectoryValidationInputs {
            cells: &cells,
            portals: &portals,
            bvh: &bvh,
            cell_locator: &locator,
            sh: ClusterDirectoryShInventory {
                octahedral: Some(&base),
                ..Default::default()
            },
        })
        .unwrap();
    assert_eq!(
        directory
            .ranges
            .iter()
            .map(|range| range.count)
            .sum::<u32>(),
        64
    );
}

#[test]
fn cluster_directory_scaled_node_closure_adds_exact_constituent_cube() {
    let dims = [8, 8, 8];
    let mut probes = vec![OctahedralShProbe::default(); 512];
    for probe in &mut probes {
        probe.node_scale = 1;
    }
    probes[0].validity = 1;
    let base = base_volume(dims, probes);
    let cells = CellsSection {
        cells: vec![CellRecord {
            bounds_min: [-0.1; 3],
            bounds_max: [0.1; 3],
            flags: CELL_FLAG_DRAWABLE,
            face_start: 0,
            face_count: 0,
            portal_ref_start: 0,
            portal_ref_count: 0,
        }],
        portal_refs: Vec::new(),
    };
    let portals = PortalsSection {
        vertices: Vec::new(),
        portals: Vec::new(),
    };
    let bvh = empty_bvh();
    let locator = CellLocatorSection {
        root: CellLocatorChild::Cell(0),
        nodes: Vec::new(),
    };
    let mut directory = one_cell_directory();
    directory.clusters[0].bounds_min = [-0.1; 3];
    directory.clusters[0].bounds_max = [0.1; 3];
    directory.resources = vec![ClusterResourceRecord {
        section_id: 34,
        domain: ClusterResourceDomain::DenseProbe,
        dimensions: dims,
    }];
    directory.ranges = vec![ClusterRangeRecord {
        resource_index: 0,
        start: 0,
        count: 512,
    }];
    directory.clusters[0].range_count = 1;
    directory
        .validate_semantics(ClusterDirectoryValidationInputs {
            cells: &cells,
            portals: &portals,
            bvh: &bvh,
            cell_locator: &locator,
            sh: ClusterDirectoryShInventory {
                octahedral: Some(&base),
                ..Default::default()
            },
        })
        .unwrap();
}

#[test]
fn cluster_directory_partial_edge_scale_zero_uses_clipped_probe_range() {
    let dims = [5, 1, 1];
    let mut probes = vec![OctahedralShProbe::default(); 5];
    probes[4].validity = 1;
    let base = base_volume(dims, probes);
    let cells = CellsSection {
        cells: vec![CellRecord {
            bounds_min: [3.9, -0.1, -0.1],
            bounds_max: [4.1, 0.1, 0.1],
            flags: CELL_FLAG_DRAWABLE,
            face_start: 0,
            face_count: 0,
            portal_ref_start: 0,
            portal_ref_count: 0,
        }],
        portal_refs: Vec::new(),
    };
    let portals = PortalsSection {
        vertices: Vec::new(),
        portals: Vec::new(),
    };
    let bvh = empty_bvh();
    let locator = CellLocatorSection {
        root: CellLocatorChild::Cell(0),
        nodes: Vec::new(),
    };
    let mut directory = one_cell_directory();
    directory.clusters[0].bounds_min = [3.9, -0.1, -0.1];
    directory.clusters[0].bounds_max = [4.1, 0.1, 0.1];
    directory.resources = vec![
        ClusterResourceRecord {
            section_id: 27,
            domain: ClusterResourceDomain::AffinityCell,
            dimensions: [2, 1, 1],
        },
        ClusterResourceRecord {
            section_id: 34,
            domain: ClusterResourceDomain::DenseProbe,
            dimensions: dims,
        },
    ];
    let sparse = DeltaShVolumesSection {
        affinity_factor: 4,
        affinity_dims: [2, 1, 1],
        tile_dimension: 6,
        tile_border: 1,
        animation_descriptor_indices: Vec::new(),
        valid_probe_masks: vec![0, 0],
        cell_levels: vec![0, 0],
        affinity_offsets: vec![0, 0, 0],
        affinity_lights: Vec::new(),
        delta_subblocks: Vec::new(),
    };
    directory.ranges = vec![
        ClusterRangeRecord {
            resource_index: 0,
            start: 1,
            count: 1,
        },
        ClusterRangeRecord {
            resource_index: 1,
            start: 4,
            count: 1,
        },
    ];
    directory.clusters[0].range_count = 2;
    directory
        .validate_semantics(ClusterDirectoryValidationInputs {
            cells: &cells,
            portals: &portals,
            bvh: &bvh,
            cell_locator: &locator,
            sh: ClusterDirectoryShInventory {
                octahedral: Some(&base),
                delta: Some(&sparse),
                ..Default::default()
            },
        })
        .unwrap();
}

#[test]
fn cluster_directory_resource_inventory_and_descriptor_dependencies_are_strict() {
    let base = base_volume([0, 0, 0], Vec::new());
    let cells = cells(1);
    let portals = PortalsSection {
        vertices: Vec::new(),
        portals: Vec::new(),
    };
    let bvh = empty_bvh();
    let locator = CellLocatorSection {
        root: CellLocatorChild::Cell(0),
        nodes: Vec::new(),
    };
    let directory = one_cell_directory();
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
    assert!(matches!(
        directory.validate_semantics(inputs),
        Err(ClusterDirectoryError::MissingResource(_))
    ));

    let mut directory = one_cell_directory();
    directory.resources.push(ClusterResourceRecord {
        section_id: 34,
        domain: ClusterResourceDomain::DenseProbe,
        dimensions: [0, 0, 0],
    });
    directory.validate_semantics(inputs).unwrap();

    let delta = DeltaShVolumesSection {
        affinity_factor: 4,
        affinity_dims: [0, 0, 0],
        tile_dimension: 6,
        tile_border: 1,
        animation_descriptor_indices: vec![0],
        valid_probe_masks: Vec::new(),
        cell_levels: Vec::new(),
        affinity_offsets: vec![0],
        affinity_lights: Vec::new(),
        delta_subblocks: Vec::new(),
    };
    directory.resources.insert(
        0,
        ClusterResourceRecord {
            section_id: 27,
            domain: ClusterResourceDomain::AffinityCell,
            dimensions: [0, 0, 0],
        },
    );
    let inputs = ClusterDirectoryValidationInputs {
        sh: ClusterDirectoryShInventory {
            octahedral: Some(&base),
            delta: Some(&delta),
            ..Default::default()
        },
        ..inputs
    };
    assert!(matches!(
        directory.validate_semantics(inputs),
        Err(ClusterDirectoryError::ResourceMismatch(_))
    ));
}

// Version 2's fourth and fifth words are gone, not zeroed.
#[test]
fn cluster_directory_range_record_is_three_words_and_a_reserved_zero() {
    let section = one_range_directory();
    let bytes = section.try_to_bytes().unwrap();
    let offset = HEADER_SIZE + CLUSTER_RECORD_SIZE + 3 * RESOURCE_RECORD_SIZE + MEMBER_RECORD_SIZE;
    let words = words_in(&bytes[offset..offset + RANGE_RECORD_SIZE]);
    assert_eq!(words, [0, 0, 1, 0]);
    let mut bad = bytes;
    bad[offset + 12..offset + 16].copy_from_slice(&1u32.to_le_bytes());
    assert!(matches!(
        ClusterDirectorySection::from_bytes(&bad),
        Err(ClusterDirectoryError::InvalidData(_))
    ));
}

/// Affinity, dense and affinity resources, so a family can name an affinity
/// one and be refused on a dense one.
fn one_range_directory() -> ClusterDirectorySection {
    let mut section = one_cell_directory();
    section.resources = vec![
        ClusterResourceRecord {
            section_id: SectionId::DeltaShVolumes as u32,
            domain: ClusterResourceDomain::AffinityCell,
            dimensions: [1, 1, 1],
        },
        ClusterResourceRecord {
            section_id: SectionId::OctahedralShVolume as u32,
            domain: ClusterResourceDomain::DenseProbe,
            dimensions: [1, 1, 1],
        },
        ClusterResourceRecord {
            section_id: SectionId::DirectShDeltaVolumes as u32,
            domain: ClusterResourceDomain::AffinityCell,
            dimensions: [1, 1, 1],
        },
    ];
    section.clusters[0].range_count = 1;
    section.ranges = vec![ClusterRangeRecord {
        resource_index: 0,
        start: 0,
        count: 1,
    }];
    section
}

/// Both of `one_range_directory`'s streamed families, as the compiler stores
/// them: `(resource_index, worst_entries, worst_tile_f16)`.
const FAMILIES: &[(u32, u32, u32)] = &[(0, 1, 2), (2, 3, 4)];

fn bound_entry(lead: u32, families: &[(u32, u32, u32)]) -> ShMandatoryBoundEntry {
    ShMandatoryBoundEntry {
        lead,
        worst_dense_slots: lead + 7,
        worst_cell: 0,
        families: families
            .iter()
            .map(
                |&(resource_index, worst_entries, worst_tile_f16)| ShMandatoryBoundFamily {
                    resource_index,
                    worst_entries,
                    worst_tile_f16,
                },
            )
            .collect(),
    }
}

/// `one_range_directory` with `entries`, encoded.
fn tail_bytes(entries: Vec<ShMandatoryBoundEntry>) -> Vec<u8> {
    let mut section = one_range_directory();
    section.mandatory_bound = entries;
    section.try_to_bytes().unwrap()
}

/// Byte offset of the tail's `bound_count` in `one_range_directory`'s encoding.
fn tail_offset() -> usize {
    let bytes = tail_bytes(Vec::new());
    bytes.len() - 4
}

fn word_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn words_in(bytes: &[u8]) -> Vec<u32> {
    (0..bytes.len())
        .step_by(4)
        .map(|offset| word_at(bytes, offset))
        .collect()
}

fn set_word(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn bound_tail_round_trips_two_entries_at_their_exact_wire_offsets() {
    let mut section = one_range_directory();
    section.mandatory_bound = vec![
        bound_entry(100, &[(0, 11, 12), (2, 13, 14)]),
        bound_entry(200, &[(0, 21, 22), (2, 23, 24)]),
    ];
    let bytes = section.try_to_bytes().unwrap();
    assert_eq!(bytes.len(), section.byte_len().unwrap());
    let tail = tail_offset();
    assert_eq!(
        bytes.len(),
        tail + 4 + 2 * (BOUND_ENTRY_HEADER_SIZE + 2 * BOUND_FAMILY_RECORD_SIZE)
    );
    let words = words_in(&bytes[tail..]);
    assert_eq!(
        words,
        [
            2, // bound_count
            100, 107, 0, 2, 0, 11, 12, 2, 13, 14, // default lead
            200, 207, 0, 2, 0, 21, 22, 2, 23, 24, // baked maximum
        ]
    );
    assert_eq!(
        ClusterDirectorySection::from_bytes(&bytes).unwrap(),
        section
    );
}

// A level with no streamed sparse family has none to name.
#[test]
fn bound_tail_encodes_zero_families_as_family_count_zero() {
    let mut section = one_range_directory();
    section
        .resources
        .retain(|resource| resource.domain == ClusterResourceDomain::DenseProbe);
    let tail = section.try_to_bytes().unwrap().len() - 4;
    section.mandatory_bound = vec![bound_entry(100, &[]), bound_entry(200, &[])];
    let bytes = section.try_to_bytes().unwrap();
    assert_eq!(bytes.len(), tail + 4 + 2 * BOUND_ENTRY_HEADER_SIZE);
    assert_eq!(word_at(&bytes, tail + 4 + 12), 0);
    assert_eq!(
        ClusterDirectorySection::from_bytes(&bytes).unwrap(),
        section
    );
}

#[test]
fn bound_tail_holds_one_entry_when_the_maximum_does_not_exceed_the_default() {
    let mut section = one_range_directory();
    section.mandatory_bound = vec![bound_entry(100, FAMILIES)];
    let bytes = section.try_to_bytes().unwrap();
    assert_eq!(word_at(&bytes, tail_offset()), 1);
    assert_eq!(
        ClusterDirectorySection::from_bytes(&bytes).unwrap(),
        section
    );
}

#[test]
fn bound_tail_holds_the_whole_map_at_lead_zero_with_the_marker_cell() {
    let mut section = one_range_directory();
    section.mandatory_bound = vec![ShMandatoryBoundEntry {
        worst_cell: WHOLE_MAP_WORST_CELL,
        ..bound_entry(0, FAMILIES)
    }];
    let bytes = section.try_to_bytes().unwrap();
    assert_eq!(word_at(&bytes, tail_offset() + 4 + 8), u32::MAX);
    assert_eq!(
        ClusterDirectorySection::from_bytes(&bytes).unwrap(),
        section
    );
}

#[test]
fn bound_tail_refuses_the_whole_map_marker_beside_another_entry_or_past_lead_zero() {
    let marker = |lead| ShMandatoryBoundEntry {
        worst_cell: WHOLE_MAP_WORST_CELL,
        ..bound_entry(lead, FAMILIES)
    };
    let mut section = one_range_directory();
    section.mandatory_bound = vec![marker(16)];
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::BoundWholeMapShape { lead: 16, .. })
    ));
    section.mandatory_bound = vec![marker(0), bound_entry(16, FAMILIES)];
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterDirectoryError::BoundWholeMapShape { entries: 2, .. })
    ));
}

// No id 34 means no ranges and no pool, so the bound has nothing to describe.
#[test]
fn directory_without_id_34_stores_bound_count_zero_and_refuses_any_entry() {
    let mut section = one_cell_directory();
    let bytes = section.try_to_bytes().unwrap();
    assert_eq!(word_at(&bytes, bytes.len() - 4), 0);
    assert_eq!(
        ClusterDirectorySection::from_bytes(&bytes).unwrap(),
        section
    );

    let cells = cells(1);
    let portals = PortalsSection {
        vertices: Vec::new(),
        portals: Vec::new(),
    };
    let bvh = empty_bvh();
    let locator = CellLocatorSection {
        root: CellLocatorChild::Cell(0),
        nodes: Vec::new(),
    };
    let inputs = ClusterDirectoryValidationInputs {
        cells: &cells,
        portals: &portals,
        bvh: &bvh,
        cell_locator: &locator,
        sh: ClusterDirectoryShInventory::default(),
    };
    section.validate_semantics(inputs).unwrap();
    section.mandatory_bound = vec![bound_entry(100, &[])];
    assert_eq!(
        section.validate_semantics(inputs),
        Err(ClusterDirectoryError::BoundWithoutProbes)
    );
}

#[test]
fn bound_tail_refuses_leads_out_of_order_by_name() {
    let mut bytes = tail_bytes(vec![bound_entry(100, FAMILIES), bound_entry(200, FAMILIES)]);
    let second_lead = tail_offset() + 4 + BOUND_ENTRY_HEADER_SIZE + 2 * BOUND_FAMILY_RECORD_SIZE;
    set_word(&mut bytes, second_lead, 100);
    assert_eq!(
        ClusterDirectorySection::from_bytes(&bytes),
        Err(ClusterDirectoryError::BoundLeadOrder)
    );
    set_word(&mut bytes, second_lead, 99);
    assert_eq!(
        ClusterDirectorySection::from_bytes(&bytes),
        Err(ClusterDirectoryError::BoundLeadOrder)
    );
}

#[test]
fn bound_tail_refuses_families_out_of_order_by_name() {
    let mut bytes = tail_bytes(vec![bound_entry(100, &[(0, 1, 2), (2, 3, 4)])]);
    let second_family = tail_offset() + 4 + BOUND_ENTRY_HEADER_SIZE + BOUND_FAMILY_RECORD_SIZE;
    set_word(&mut bytes, second_family, 0);
    assert_eq!(
        ClusterDirectorySection::from_bytes(&bytes),
        Err(ClusterDirectoryError::BoundFamilyOrder { entry: 0 })
    );
}

#[test]
fn bound_tail_refuses_a_family_outside_the_streamed_resources_by_name() {
    let second_family = tail_offset() + 4 + BOUND_ENTRY_HEADER_SIZE + BOUND_FAMILY_RECORD_SIZE;
    let mut bytes = tail_bytes(vec![bound_entry(100, FAMILIES)]);
    // Resource 1 is dense; resource 3 is past the table.
    for resource in [1, 3] {
        set_word(&mut bytes, second_family, resource);
        assert_eq!(
            ClusterDirectorySection::from_bytes(&bytes),
            Err(ClusterDirectoryError::BoundFamilyResource {
                entry: 0,
                resource,
                count: 3
            })
        );
    }
}

// Regression: id 48 passed as an affinity resource, and the runtime then failed
// a whole-resident family as a capacity problem.
#[test]
fn bound_tail_refuses_the_whole_resident_id_48_family_by_name() {
    let mut section = one_range_directory();
    section.resources.push(ClusterResourceRecord {
        section_id: SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32,
        domain: ClusterResourceDomain::AffinityCell,
        dimensions: [1, 1, 1],
    });
    section.mandatory_bound = vec![bound_entry(100, &[(0, 1, 2), (2, 3, 4), (3, 5, 6)])];
    assert_eq!(
        section.validate_structure(),
        Err(ClusterDirectoryError::BoundFamilyResource {
            entry: 0,
            resource: 3,
            count: 4
        })
    );
    section.mandatory_bound = vec![bound_entry(100, FAMILIES)];
    assert_eq!(section.validate_structure(), Ok(()));
}

// Regression: an omitted family read as a zero bound at load.
#[test]
fn bound_tail_refuses_an_omitted_streamed_family_by_name() {
    let mut section = one_range_directory();
    section.mandatory_bound = vec![bound_entry(100, &[(0, 1, 2)])];
    assert_eq!(
        section.validate_structure(),
        Err(ClusterDirectoryError::BoundFamilyMissing {
            entry: 0,
            section_id: SectionId::DirectShDeltaVolumes as u32
        })
    );
}

#[test]
fn bound_tail_refuses_a_worst_cell_past_the_cell_table_by_name() {
    let mut bytes = tail_bytes(vec![bound_entry(100, FAMILIES)]);
    set_word(&mut bytes, tail_offset() + 4 + 8, 1);
    assert_eq!(
        ClusterDirectorySection::from_bytes(&bytes),
        Err(ClusterDirectoryError::BoundCellOutOfRange {
            entry: 0,
            cell: 1,
            count: 1
        })
    );
}

#[test]
fn bound_tail_refuses_a_count_past_the_two_stored_leads_before_reading_entries() {
    let mut bytes = tail_bytes(Vec::new());
    set_word(&mut bytes, tail_offset(), u32::MAX);
    assert_eq!(
        ClusterDirectorySection::from_bytes(&bytes),
        Err(ClusterDirectoryError::BoundCount {
            count: u32::MAX,
            limit: MAX_BOUND_ENTRIES
        })
    );
    let mut section = one_range_directory();
    section.mandatory_bound = vec![
        bound_entry(1, &[]),
        bound_entry(2, &[]),
        bound_entry(3, &[]),
    ];
    assert!(matches!(
        section.try_to_bytes(),
        Err(ClusterDirectoryError::BoundCount { count: 3, .. })
    ));
}

// Every cut through the tail must be refused, never read past the section.
#[test]
fn bound_tail_refuses_every_truncation_by_name() {
    let bytes = tail_bytes(vec![bound_entry(100, FAMILIES), bound_entry(200, FAMILIES)]);
    let tail = tail_offset();
    for length in tail..bytes.len() {
        assert!(
            matches!(
                ClusterDirectorySection::from_bytes(&bytes[..length]),
                Err(ClusterDirectoryError::BoundTruncated { .. })
            ),
            "cut at {length} of {}",
            bytes.len()
        );
    }
    assert!(matches!(
        ClusterDirectorySection::from_bytes(&bytes[..tail - 1]),
        Err(ClusterDirectoryError::InvalidData(_))
    ));
}

#[test]
fn bound_tail_refuses_a_forged_family_count_without_allocating_it() {
    let mut bytes = tail_bytes(vec![bound_entry(100, FAMILIES)]);
    set_word(&mut bytes, tail_offset() + 4 + 12, u32::MAX);
    assert!(matches!(
        ClusterDirectorySection::from_bytes(&bytes),
        Err(ClusterDirectoryError::BoundTruncated { .. })
    ));
}

#[test]
fn bound_tail_refuses_bytes_past_the_last_entry_by_name() {
    let mut bytes = tail_bytes(vec![bound_entry(100, FAMILIES)]);
    bytes.extend_from_slice(&[0; 4]);
    assert_eq!(
        ClusterDirectorySection::from_bytes(&bytes),
        Err(ClusterDirectoryError::BoundTrailingBytes { extra: 4 })
    );
}

// The loader's `old_directory_version_is_refused_by_name` covers this check and
// the container check together, through a real file.
#[test]
fn decoder_names_a_previous_format_directory_and_the_fix() {
    assert_eq!(CLUSTER_DIRECTORY_VERSION, 3);
    assert_eq!(CLUSTER_DIRECTORY_CONTAINER_VERSION, 3);
    let mut bytes = tail_bytes(Vec::new());
    set_word(&mut bytes, 0, 2);
    let error = ClusterDirectorySection::from_bytes(&bytes).unwrap_err();
    assert_eq!(
        error,
        ClusterDirectoryError::VersionMismatch {
            version: 2,
            expected: CLUSTER_DIRECTORY_VERSION
        }
    );
    let message = error.to_string();
    assert!(
        message.contains("ClusterDirectoryVersionMismatch"),
        "{message}"
    );
    assert!(message.contains("re-bake the level"), "{message}");
}
