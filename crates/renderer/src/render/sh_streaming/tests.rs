//! Focused CPU-only streamed-residency lifecycle tests.

use super::*;
use postretro_level_format::cluster_directory::{
    ClusterRangeRecord, ClusterRangeRole, ClusterRecord, ClusterResourceDomain,
    ClusterResourceRecord,
};
use postretro_level_format::cluster_sh_payloads::{DecodedClusterShBlock, DecodedClusterShPayload};
use postretro_level_format::sh_volume::OctahedralShProbe;

fn one_l0_brick() -> postretro_level_loader::ShStreamBaseMetadata {
    postretro_level_loader::ShStreamBaseMetadata {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: [4, 4, 4],
        probe_stride: 8,
        tile_dimension: 4,
        tile_border: 1,
        atlas_dimensions: [8, 8],
        layer_count: 1,
        tiles_per_layer: 1,
        atlas_tiles_per_row: 1,
        irradiance_format: 0,
        probes: vec![
            OctahedralShProbe {
                validity: 1,
                density_level: 0,
                node_scale: 0,
                ..Default::default()
            };
            64
        ],
        animation_descriptors: Vec::new(),
        slot_for_map_light: Vec::new(),
    }
}

fn cluster(range_start: u32) -> ClusterRecord {
    cluster_with_ranges(range_start, 1)
}

fn cluster_with_ranges(range_start: u32, range_count: u32) -> ClusterRecord {
    ClusterRecord {
        bounds_min: [0.0; 3],
        bounds_max: [1.0; 3],
        member_start: 0,
        member_count: 0,
        range_start,
        range_count,
        primitive_count: 0,
        flags: 0,
    }
}

fn sparse_source(
    section_id: u32,
    animation_descriptor_indices: Vec<u32>,
) -> postretro_level_loader::ShStreamSparseMetadata {
    postretro_level_loader::ShStreamSparseMetadata {
        section_id,
        internal_version: 1,
        affinity_dims: [1, 1, 1],
        tile_dimension: 4,
        tile_border: 1,
        valid_probe_masks: vec![1],
        cell_levels: vec![0],
        affinity_offsets: vec![0, 1],
        affinity_lights: vec![0],
        animation_descriptor_indices,
    }
}

fn direct_metadata() -> postretro_level_loader::ShStreamDirectMetadata {
    postretro_level_loader::ShStreamDirectMetadata {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: [4, 4, 4],
        tile_dimension: 4,
        tile_border: 1,
        atlas_dimensions: [8, 8],
        layer_count: 1,
        tiles_per_layer: 1,
        atlas_tiles_per_row: 1,
        irradiance_format: 0,
    }
}

fn all_sparse_sources() -> postretro_level_loader::ShStreamSourceMetadata {
    postretro_level_loader::ShStreamSourceMetadata {
        indirect_delta: Some(sparse_source(INDIRECT_DELTA_ID, vec![0])),
        direct: Some(direct_metadata()),
        direct_delta: Some(sparse_source(DIRECT_DELTA_ID, Vec::new())),
        animated_direct_delta: Some(sparse_source(ANIMATED_DIRECT_DELTA_ID, vec![0])),
    }
}

fn range(
    resource_index: u32,
    start: u32,
    count: u32,
    owner_cluster_id: u32,
    role: ClusterRangeRole,
) -> ClusterRangeRecord {
    ClusterRangeRecord {
        resource_index,
        start,
        count,
        owner_cluster_id,
        role,
    }
}

fn state_with_clusters(cluster_count: u32) -> ShResidencyState {
    let directory = ClusterDirectorySection {
        runtime_cell_count: 0,
        primitive_limit: 0,
        cell_limit: 0,
        clusters: (0..cluster_count).map(cluster).collect(),
        resources: vec![ClusterResourceRecord {
            section_id: INDIRECT_BASE_ID,
            domain: ClusterResourceDomain::DenseProbe,
            dimensions: [4, 4, 4],
        }],
        members: Vec::new(),
        ranges: (0..cluster_count)
            .map(|cluster_id| ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 64,
                owner_cluster_id: cluster_id,
                role: ClusterRangeRole::Owned,
            })
            .collect(),
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    };
    ShResidencyState::from_parts(
        [9; 32],
        &directory,
        &one_l0_brick(),
        &postretro_level_loader::ShStreamSourceMetadata::default(),
    )
    .expect("minimal dense directory is valid")
}

fn prepared(cluster_id: u32, generation: u64, content_tag: [u8; 32]) -> PreparedShCluster {
    PreparedShCluster {
        generation,
        content_tag,
        chunk: DecodedClusterShPayload {
            cluster_id,
            bytes: Vec::new(),
            blocks: Vec::new(),
        },
    }
}

#[test]
fn dense_patch_owner_is_the_lowest_cluster_containing_the_probe() {
    let directory = ClusterDirectorySection {
        runtime_cell_count: 0,
        primitive_limit: 0,
        cell_limit: 0,
        clusters: vec![cluster(0), cluster(1)],
        resources: vec![ClusterResourceRecord {
            section_id: INDIRECT_BASE_ID,
            domain: ClusterResourceDomain::DenseProbe,
            dimensions: [4, 4, 4],
        }],
        members: Vec::new(),
        ranges: vec![
            // A lower-id halo is still the canonical dense-patch writer.
            // Unlike sparse CSR rows, dense ownership is deliberately
            // derived from the lowest containing cluster, not id-49's
            // owner field.
            ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 64,
                owner_cluster_id: 1,
                role: ClusterRangeRole::Halo,
            },
            ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 64,
                owner_cluster_id: 1,
                role: ClusterRangeRole::Owned,
            },
        ],
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    };
    let state = ShResidencyState::from_parts(
        [7; 32],
        &directory,
        &one_l0_brick(),
        &postretro_level_loader::ShStreamSourceMetadata::default(),
    )
    .unwrap();

    assert_eq!(state.dense_owner[0], Some(0));
    assert_eq!(state.nodes_by_owner[&0].len(), 1);
}

#[test]
fn sparse_owner_halo_ranges_keep_the_writer_for_ids_27_41_and_45() {
    let resources = vec![
        ClusterResourceRecord {
            section_id: INDIRECT_BASE_ID,
            domain: ClusterResourceDomain::DenseProbe,
            dimensions: [4, 4, 4],
        },
        ClusterResourceRecord {
            section_id: INDIRECT_DELTA_ID,
            domain: ClusterResourceDomain::AffinityCell,
            dimensions: [1, 1, 1],
        },
        ClusterResourceRecord {
            section_id: DIRECT_DELTA_ID,
            domain: ClusterResourceDomain::AffinityCell,
            dimensions: [1, 1, 1],
        },
        ClusterResourceRecord {
            section_id: ANIMATED_DIRECT_DELTA_ID,
            domain: ClusterResourceDomain::AffinityCell,
            dimensions: [1, 1, 1],
        },
    ];
    let directory = ClusterDirectorySection {
        runtime_cell_count: 0,
        primitive_limit: 0,
        cell_limit: 0,
        clusters: vec![cluster_with_ranges(0, 4), cluster_with_ranges(4, 4)],
        resources,
        members: Vec::new(),
        ranges: vec![
            range(0, 0, 64, 1, ClusterRangeRole::Halo),
            range(1, 0, 1, 1, ClusterRangeRole::Halo),
            range(2, 0, 1, 1, ClusterRangeRole::Halo),
            range(3, 0, 1, 1, ClusterRangeRole::Halo),
            range(0, 0, 64, 1, ClusterRangeRole::Owned),
            range(1, 0, 1, 1, ClusterRangeRole::Owned),
            range(2, 0, 1, 1, ClusterRangeRole::Owned),
            range(3, 0, 1, 1, ClusterRangeRole::Owned),
        ],
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    };

    let state =
        ShResidencyState::from_parts([11; 32], &directory, &one_l0_brick(), &all_sparse_sources())
            .expect("overlapping sparse ownership is valid");

    for section_id in [INDIRECT_DELTA_ID, DIRECT_DELTA_ID, ANIMATED_DIRECT_DELTA_ID] {
        assert_eq!(state.sparse_row_owner[&(section_id, 0)], 1);
    }
    // The lower-id halo must wait for each sparse canonical writer. Dense
    // patch ownership is deliberately different and remains lower-id.
    assert_eq!(state.dense_owner[0], Some(0));
    assert!(state.owner_dependencies[0].contains(&1));
}

#[test]
fn dense_patch_writer_waits_for_a_different_stored_node_owner() {
    let directory = ClusterDirectorySection {
        runtime_cell_count: 0,
        primitive_limit: 0,
        cell_limit: 0,
        clusters: vec![cluster_with_ranges(0, 1), cluster_with_ranges(1, 1)],
        resources: vec![ClusterResourceRecord {
            section_id: INDIRECT_BASE_ID,
            domain: ClusterResourceDomain::DenseProbe,
            dimensions: [4, 4, 4],
        }],
        members: Vec::new(),
        ranges: vec![
            range(0, 0, 1, 0, ClusterRangeRole::Owned),
            range(0, 1, 63, 1, ClusterRangeRole::Owned),
        ],
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    };
    let state = ShResidencyState::from_parts(
        [12; 32],
        &directory,
        &one_l0_brick(),
        &postretro_level_loader::ShStreamSourceMetadata::default(),
    )
    .expect("one stored node can contain patches owned by distinct clusters");

    let node = state.dense_node[1].expect("second valid probe has a stored node");
    assert_eq!(state.dense_owner[1], Some(1));
    assert_eq!(state.node_owner[&node], 0);
    assert!(state.owner_dependencies[1].contains(&0));
}

#[test]
fn base_only_manifest_does_not_require_an_id35_shared_slot_member() {
    let state = state_with_clusters(1);
    assert!(!state.direct_required);
    assert!(!state.direct_compose_required);
    assert!(state.direct_promotion_resident_rows.is_empty());
    assert!(state.direct_animated_resident_rows.is_empty());
}

#[test]
fn paired_direct_manifest_refuses_a_chunk_missing_id35() {
    let state = ShResidencyState::from_parts(
        [13; 32],
        &ClusterDirectorySection {
            runtime_cell_count: 0,
            primitive_limit: 0,
            cell_limit: 0,
            clusters: vec![cluster(0)],
            resources: vec![ClusterResourceRecord {
                section_id: INDIRECT_BASE_ID,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: [4, 4, 4],
            }],
            members: Vec::new(),
            ranges: vec![range(0, 0, 64, 0, ClusterRangeRole::Owned)],
            seam_portal_ids: Vec::new(),
            cluster_hints: Vec::new(),
        },
        &one_l0_brick(),
        &all_sparse_sources(),
    )
    .expect("paired direct manifest metadata is valid");

    // The id-34 member and patch block are structurally valid, isolating the
    // missing id-35 member as the only failure. A paired manifest must never
    // publish a partial shared-slot cluster.
    let mut bytes = vec![0; 36];
    bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
    bytes[8..12].copy_from_slice(&1_u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&1_u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&1_u32.to_le_bytes());
    let chunk = DecodedClusterShPayload {
        cluster_id: 0,
        bytes,
        blocks: vec![
            DecodedClusterShBlock {
                section_id: INDIRECT_BASE_ID,
                kind: ISOLATED_ATLAS_BLOCK,
                element_count: 1,
                body: 0..20,
            },
            DecodedClusterShBlock {
                section_id: INDIRECT_BASE_ID,
                kind: PROBE_PATCH_BLOCK,
                element_count: 1,
                body: 20..36,
            },
        ],
    };

    assert_eq!(
        state.validate_isolated_atlases(0, &chunk),
        Err(ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "chunk omits required id-35 member of the shared dense pool",
        })
    );
}

#[test]
fn coalesced_row_union_drops_a_row_after_its_last_contributor() {
    let mut state = ShResidencyState {
        // This fixture exercises only the row-union mirrors. Building a
        // complete directory would obscure the eviction invariant.
        content_tag: [0; 32],
        base_metadata: one_l0_brick(),
        source_metadata: Default::default(),
        cluster_count: 0,
        grid_dimensions: [4, 4, 4],
        generation: 0,
        targets: BTreeSet::new(),
        dense_owner: Vec::new(),
        dense_node: Vec::new(),
        dense_node_local_slot: Vec::new(),
        node_layouts: BTreeMap::new(),
        node_owner: BTreeMap::new(),
        nodes_by_owner: BTreeMap::new(),
        node_slots: BTreeMap::new(),
        owner_dependencies: Vec::new(),
        sparse_row_owner: BTreeMap::new(),
        sparse_capacity_floors: BTreeMap::new(),
        dense_slots: FirstFitRanges::default(),
        sparse_pools: BTreeMap::new(),
        compose_words: Vec::new(),
        sampled_words: Vec::new(),
        installed: BTreeMap::new(),
        sampleable: BTreeSet::new(),
        pending_promotion: BTreeSet::new(),
        dirty_rows: BTreeSet::new(),
        indirect_dirty_rows: BTreeSet::new(),
        indirect_resident_rows: BTreeSet::new(),
        indirect_base_row_refs: BTreeMap::from([(2, 1)]),
        indirect_delta_row_refs: BTreeMap::from([(2, 1)]),
        direct_promotion_dirty_rows: BTreeSet::new(),
        direct_animated_dirty_rows: BTreeSet::new(),
        direct_promotion_resident_rows: BTreeSet::new(),
        direct_animated_resident_rows: BTreeSet::new(),
        direct_base_row_refs: BTreeMap::new(),
        direct_promotion_row_refs: BTreeMap::new(),
        direct_animated_row_refs: BTreeMap::new(),
        direct_required: false,
        direct_compose_required: false,
        direct_animation_descriptor_indices: Vec::new(),
        indirect_was_active: false,
        last_indirect_mask: LightTermMask::ALL,
        direct_was_active: false,
        last_direct_mask: LightTermMask::ALL,
        generation_has_reset: false,
        indirect_compose_epoch: 0,
        direct_compose_epoch: 0,
        install_cpu: InstallCpuCounters::default(),
        gpu: None,
    };
    state.refresh_indirect_resident_rows();
    assert_eq!(state.indirect_resident_rows, BTreeSet::from([2]));
    decrement_row_ref(&mut state.indirect_base_row_refs, 2).unwrap();
    state.refresh_indirect_resident_rows();
    assert_eq!(state.indirect_resident_rows, BTreeSet::from([2]));
    decrement_row_ref(&mut state.indirect_delta_row_refs, 2).unwrap();
    state.refresh_indirect_resident_rows();
    assert!(state.indirect_resident_rows.is_empty());
    state.indirect_dirty_rows.insert(2);
    state.direct_promotion_dirty_rows.insert(4);
    state.direct_animated_dirty_rows.insert(4);
    assert_eq!(state.snapshot().dirty_affinity_rows, 2);
}

#[test]
fn sparse_floor_covers_the_sum_of_a_cluster_owned_rows() {
    let source = postretro_level_loader::ShStreamSparseMetadata {
        section_id: INDIRECT_DELTA_ID,
        internal_version: 1,
        affinity_dims: [2, 1, 1],
        tile_dimension: 4,
        tile_border: 1,
        valid_probe_masks: vec![1, 1],
        cell_levels: vec![2, 2],
        affinity_offsets: vec![0, 2, 5],
        affinity_lights: vec![1; 5],
        animation_descriptor_indices: Vec::new(),
    };
    let sources = postretro_level_loader::ShStreamSourceMetadata {
        indirect_delta: Some(source),
        ..Default::default()
    };
    let owners = BTreeMap::from([((INDIRECT_DELTA_ID, 0), 3), ((INDIRECT_DELTA_ID, 1), 3)]);
    let floors = sparse_capacity_floors(&sources, &owners).unwrap();

    // The old largest-row rule would have produced four entries. Both
    // rows are owned by cluster 3, so its first install needs 2 + 3 plus
    // the missing-row sentinel.
    assert_eq!(floors[&INDIRECT_DELTA_ID].0, 6);
    assert_eq!(floors[&INDIRECT_DELTA_ID].1 % 2, 0);
}

#[test]
fn stale_generation_cannot_roll_back_an_active_streaming_session() {
    assert_eq!(validate_generation_transition(4, 5, true), Ok(true));
    assert_eq!(
        validate_generation_transition(5, 4, true),
        Err(ShResidencyDrainError::StaleGeneration {
            current: 5,
            received: 4,
        })
    );
}

#[test]
fn malformed_target_transition_leaves_the_live_session_unchanged() {
    let mut state = state_with_clusters(2);
    state.generation = 5;
    state.generation_has_reset = true;
    state.targets = BTreeSet::from([0]);

    let malformed_reset = ShDrainBatch {
        generation: 6,
        content_tag: state.content_tag,
        // Bit two is outside this two-cluster generation. The renderer
        // must reject it before a new-generation clear can run.
        target_reset: Some(vec![1 << 2]),
        ..ShDrainBatch::default()
    };
    assert_eq!(
        state.plan_target_transition(&malformed_reset),
        Err(ShResidencyDrainError::InvalidTargetBitset)
    );
    assert_eq!(state.generation, 5);
    assert_eq!(state.targets, BTreeSet::from([0]));
    assert!(state.generation_has_reset);

    let conflicting_delta = ShDrainBatch {
        generation: 5,
        content_tag: state.content_tag,
        target_add: vec![1],
        target_remove: vec![1],
        ..ShDrainBatch::default()
    };
    assert_eq!(
        state.plan_target_transition(&conflicting_delta),
        Err(ShResidencyDrainError::DuplicateTargetDelta(1))
    );
    assert_eq!(state.targets, BTreeSet::from([0]));
}

#[test]
fn over_cap_ready_batch_is_rejected_before_renderer_state_changes() {
    let mut state = state_with_clusters(3);
    state.generation = 5;
    state.generation_has_reset = true;
    state.targets = BTreeSet::from([0]);
    let batch = ShDrainBatch {
        generation: 5,
        content_tag: state.content_tag,
        target_add: vec![1],
        ready: vec![
            prepared(0, 5, state.content_tag),
            prepared(1, 5, state.content_tag),
            prepared(2, 5, state.content_tag),
        ],
        ..ShDrainBatch::default()
    };

    let error = state.validate_batch_contract(&batch).unwrap_err();
    assert!(matches!(error, ShResidencyDrainError::InvalidBatch(_)));
    assert_eq!(state.generation, 5);
    assert_eq!(state.targets, BTreeSet::from([0]));
    assert!(state.installed.is_empty());
    assert!(state.pending_promotion.is_empty());
}

#[test]
fn duplicate_ready_ids_are_rejected_before_renderer_state_changes() {
    let mut state = state_with_clusters(2);
    state.generation = 8;
    state.generation_has_reset = true;
    state.targets = BTreeSet::from([0]);
    let batch = ShDrainBatch {
        generation: 8,
        content_tag: state.content_tag,
        target_add: vec![1],
        ready: vec![
            prepared(1, 8, state.content_tag),
            prepared(1, 8, state.content_tag),
        ],
        ..ShDrainBatch::default()
    };

    let error = state.validate_batch_contract(&batch).unwrap_err();
    assert!(matches!(error, ShResidencyDrainError::InvalidBatch(_)));
    assert_eq!(state.generation, 8);
    assert_eq!(state.targets, BTreeSet::from([0]));
    assert!(state.installed.is_empty());
    assert!(state.pending_promotion.is_empty());
}

#[test]
fn reset_latch_allows_multiple_same_generation_target_deltas() {
    let mut state = state_with_clusters(2);
    state.generation = 7;
    state.generation_has_reset = true;
    state.targets = BTreeSet::from([0]);

    let first = ShDrainBatch {
        generation: 7,
        content_tag: state.content_tag,
        target_add: vec![1],
        ..ShDrainBatch::default()
    };
    let (_, next) = state.plan_target_transition(&first).unwrap();
    state.targets = next;
    // Applying a delta without a reset intentionally leaves this
    // generation-scoped latch set; a second delta is valid.
    assert!(state.generation_has_reset);
    let second = ShDrainBatch {
        generation: 7,
        content_tag: state.content_tag,
        target_remove: vec![0],
        ..ShDrainBatch::default()
    };
    let (_, next) = state.plan_target_transition(&second).unwrap();
    assert_eq!(next, BTreeSet::from([1]));
}

#[test]
fn only_retiring_generation_pressure_is_deferred() {
    assert!(
        ShResidencyDrainError::GpuRetirementPressure { family: "id-27" }
            .is_retryable_retirement_pressure()
    );
    assert!(
        !ShResidencyDrainError::GpuCapacity {
            reason: "adapter storage binding limit"
        }
        .is_retryable_retirement_pressure()
    );
}

#[test]
fn halo_only_closure_requires_no_unrecorded_compose_epoch() {
    // A dependent can retain a complete id34/id35 closure and sparse halo
    // block without publishing any canonical patch or CSR pair. It must
    // promote at the current epoch rather than wait forever for work that
    // was correctly skipped.
    assert_eq!(
        required_compose_epochs(11, 13, true, false, []),
        Ok((11, 13))
    );
    assert_eq!(
        required_compose_epochs(11, 13, true, true, [INDIRECT_DELTA_ID]),
        Ok((12, 14))
    );
}

#[test]
fn all_family_writes_wait_for_both_next_compose_epochs_before_sampleability() {
    let (required_indirect_epoch, required_direct_epoch) = required_compose_epochs(
        41,
        71,
        true,
        true,
        [INDIRECT_DELTA_ID, DIRECT_DELTA_ID, ANIMATED_DIRECT_DELTA_ID],
    )
    .expect("validated compose epochs must advance");
    assert_eq!((required_indirect_epoch, required_direct_epoch), (42, 72));

    let installed = InstalledCluster {
        patches: Vec::new(),
        owned_nodes: Vec::new(),
        sparse_rows: Vec::new(),
        required_indirect_epoch,
        required_direct_epoch,
    };
    let no_dependencies = BTreeSet::new();

    // The installation drain starts at 41/71. A later drain can sample only
    // after both the indirect (id34/id27) and direct (id35/id41/id45)
    // compose passes have completed their required next epoch.
    assert!(!rows::installed_cluster_is_sampleable(
        &installed,
        &no_dependencies,
        &BTreeSet::new(),
        41,
        71,
    ));
    assert!(!rows::installed_cluster_is_sampleable(
        &installed,
        &no_dependencies,
        &BTreeSet::new(),
        42,
        71,
    ));
    assert!(!rows::installed_cluster_is_sampleable(
        &installed,
        &no_dependencies,
        &BTreeSet::new(),
        41,
        72,
    ));
    assert!(rows::installed_cluster_is_sampleable(
        &installed,
        &no_dependencies,
        &BTreeSet::new(),
        42,
        72,
    ));
}

#[test]
fn promotion_waits_for_the_next_drain_after_all_required_compose_passes() {
    let installed = InstalledCluster {
        patches: Vec::new(),
        owned_nodes: Vec::new(),
        sparse_rows: Vec::new(),
        required_indirect_epoch: 8,
        required_direct_epoch: 5,
    };
    let dependencies = BTreeSet::from([3]);

    // The install drain recorded work, but sampled indirection must retain its
    // old contents until both passes have encoded and the owner is sampleable
    // in the next drain.
    assert!(!rows::installed_cluster_is_sampleable(
        &installed,
        &dependencies,
        &BTreeSet::new(),
        8,
        5,
    ));
    assert!(!rows::installed_cluster_is_sampleable(
        &installed,
        &dependencies,
        &BTreeSet::from([3]),
        7,
        5,
    ));
    assert!(rows::installed_cluster_is_sampleable(
        &installed,
        &dependencies,
        &BTreeSet::from([3]),
        8,
        5,
    ));
}

#[test]
fn promotion_sweep_defers_clusters_installed_after_its_drain_snapshot() {
    let mut pending = BTreeSet::from([3]);
    let this_drain = rows::promotion_sweep_candidates(&pending);
    pending.insert(4);

    assert_eq!(this_drain, vec![3]);
    assert_eq!(rows::promotion_sweep_candidates(&pending), vec![3, 4]);
}

#[test]
fn dense_eviction_clears_compose_and_sampled_words_before_slot_reuse() {
    let mut slots = FirstFitRanges::default();
    let released = slots.allocate(1).unwrap();
    let stale_word = PROBE_INDIRECTION_VALID_BIT | (released.start << PROBE_INDIRECTION_SLOT_SHIFT);
    let mut compose_words = vec![0, stale_word];
    let mut sampled_words = compose_words.clone();
    assert_eq!(
        compose_words[1] >> PROBE_INDIRECTION_SLOT_SHIFT,
        released.start
    );

    // This is the same ordering used by `ShResidencyState::evict`: clear both
    // reachable words first, then return the slot for a later ready cluster.
    rows::invalidate_dense_words(&mut compose_words, &mut sampled_words, 1).unwrap();
    assert_eq!(compose_words[1], 0);
    assert_eq!(sampled_words[1], 0);
    slots.release(released).unwrap();

    assert_eq!(slots.allocate(1).unwrap(), released);
    assert_eq!(compose_words[1], 0);
    assert_eq!(sampled_words[1], 0);
}
