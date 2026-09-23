//! Focused CPU-only streamed-residency lifecycle tests.

use super::*;
use postretro_level_format::cluster_directory::{
    ClusterRangeRecord, ClusterRecord, ClusterResourceRecord,
};
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
    ClusterRecord {
        bounds_min: [0.0; 3],
        bounds_max: [1.0; 3],
        member_start: 0,
        member_count: 0,
        range_start,
        range_count: 1,
        primitive_count: 0,
        flags: 0,
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
    };
    ShResidencyState::from_parts(
        [9; 32],
        &directory,
        &one_l0_brick(),
        &postretro_level_loader::ShStreamSourceMetadata::default(),
    )
    .expect("minimal dense directory is valid")
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
