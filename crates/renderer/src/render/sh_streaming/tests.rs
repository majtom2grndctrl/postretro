//! Focused CPU-only streamed-residency lifecycle tests.

use super::*;
use postretro_level_format::cluster_directory::{
    ClusterRangeRecord, ClusterRecord, ClusterResourceDomain, ClusterResourceRecord,
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

fn range(resource_index: u32, start: u32, count: u32) -> ClusterRangeRecord {
    ClusterRangeRecord {
        resource_index,
        start,
        count,
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
            .map(|_| ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 64,
            })
            .collect(),
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
        mandatory_bound: crate::render::sh_streaming::whole_map_bound_for_test(),
    };
    ShResidencyState::from_parts(
        [9; 32],
        &directory,
        &one_l0_brick(),
        &postretro_level_loader::ShStreamSourceMetadata::default(),
        &[],
    )
    .expect("minimal dense directory is valid")
}

/// C.1: a level whose id 57 gates direct terms, with id 35 but neither id 41
/// nor id 45, still plans the direct passes: the base-only direct carrier,
/// so dirty rows reach Pass A. Without id 35 nothing changes.
#[test]
fn gated_direct_records_plan_the_direct_passes_without_ids_41_and_45() {
    let directory = ClusterDirectorySection {
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
        ranges: vec![range(0, 0, 64)],
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
        mandatory_bound: crate::render::sh_streaming::whole_map_bound_for_test(),
    };
    let base = one_l0_brick();
    let no_deltas = postretro_level_loader::ShStreamSourceMetadata::default();
    let plan = || {
        plan_sh_residency_parts([9; 32], &directory, &base, &no_deltas, &[], None)
            .expect("minimal dense directory is valid")
    };
    let direct_only = postretro_level_loader::ShStreamSourceMetadata {
        direct: Some(direct_metadata()),
        ..Default::default()
    };

    let ungated = ShResidencyState::from_plan(plan(), &base, &direct_only);
    assert!(
        !ungated.direct_compose_required,
        "no gated record, no delta: no pass"
    );

    let mut gated = plan();
    gate_direct_compose(&mut gated, &base, &direct_only).unwrap();
    let state = ShResidencyState::from_plan(gated, &base, &direct_only);
    assert!(state.direct_compose_required);
    assert!(
        state
            .sparse_compose
            .direct
            .as_ref()
            .is_some_and(|layout| !layout.source_present),
        "the base-only carrier: no id-41 rows"
    );

    let mut no_direct_base = plan();
    gate_direct_compose(&mut no_direct_base, &base, &no_deltas).unwrap();
    assert!(
        no_direct_base.sparse_compose.direct.is_none(),
        "no id 35, nothing to gate"
    );
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
fn overlapping_dense_ranges_are_both_named_and_counted_in_each_clusters_demand() {
    // Two clusters naming the same brick: neither is a writer in the plan, and
    // each cluster's own demand covers the whole node.
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
        ranges: vec![range(0, 0, 64), range(0, 0, 64)],
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
        mandatory_bound: crate::render::sh_streaming::whole_map_bound_for_test(),
    };
    let state = ShResidencyState::from_parts(
        [7; 32],
        &directory,
        &one_l0_brick(),
        &postretro_level_loader::ShStreamSourceMetadata::default(),
        &[],
    )
    .unwrap();

    let mut naming = Vec::new();
    state.dense_spans.any_cluster_naming(0, |cluster| {
        naming.push(cluster);
        false
    });
    naming.sort_unstable();
    assert_eq!(naming, [0, 1]);
    let tiles = state.node_layouts[&state.dense_node[0].unwrap()].tile_count;
    assert_eq!(state.whole_dense_slots, tiles);
}

#[test]
fn sparse_halo_ranges_price_every_family_from_each_clusters_own_rows() {
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
            range(0, 0, 64),
            range(1, 0, 1),
            range(2, 0, 1),
            range(3, 0, 1),
            range(0, 0, 64),
            range(1, 0, 1),
            range(2, 0, 1),
            range(3, 0, 1),
        ],
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
        mandatory_bound: crate::render::sh_streaming::whole_map_bound_for_test(),
    };

    let state = ShResidencyState::from_parts(
        [11; 32],
        &directory,
        &one_l0_brick(),
        &all_sparse_sources(),
        &[],
    )
    .expect("overlapping sparse ranges are valid");

    // A halo-only cluster installs alone, so it names the row in every family
    // and growth can price it with nothing else resident.
    for storage in &state.cluster_storage {
        for section_id in [INDIRECT_DELTA_ID, DIRECT_DELTA_ID, ANIMATED_DIRECT_DELTA_ID] {
            assert!(
                storage
                    .sparse
                    .iter()
                    .any(|(id, rows)| *id == section_id && rows.contains(&0))
            );
        }
    }
}

#[test]
fn one_stored_node_can_serve_clusters_naming_distinct_probes() {
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
        ranges: vec![range(0, 0, 1), range(0, 1, 63)],
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
        mandatory_bound: crate::render::sh_streaming::whole_map_bound_for_test(),
    };
    let state = ShResidencyState::from_parts(
        [12; 32],
        &directory,
        &one_l0_brick(),
        &postretro_level_loader::ShStreamSourceMetadata::default(),
        &[],
    )
    .expect("one stored node can contain probes named by distinct clusters");

    // Two clusters, one node: the whole map counts it once.
    let node = state.dense_node[1].expect("second valid probe has a stored node");
    assert_eq!(state.dense_node[0], Some(node));
    assert_eq!(
        state.whole_dense_slots,
        state.node_layouts[&node].tile_count
    );
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
            ranges: vec![range(0, 0, 64)],
            seam_portal_ids: Vec::new(),
            cluster_hints: Vec::new(),
            mandatory_bound: crate::render::sh_streaming::whole_map_bound_for_test(),
        },
        &one_l0_brick(),
        &all_sparse_sources(),
        &[],
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
    // Highest row id this fixture's tables reference is 4 (direct_promotion_dirty_rows /
    // direct_animated_dirty_rows insert row 4 below); capacity must cover 0..=4.
    const ROW_SPACE: usize = 5;
    let mut state = ShResidencyState {
        // This fixture exercises only the row-union mirrors. Building a
        // complete directory would obscure the eviction invariant.
        content_tag: [0; 32],
        base_metadata: one_l0_brick(),
        source_metadata: Default::default(),
        cluster_count: 0,
        grid_dimensions: [4, 4, 4],
        generation: 0,
        #[cfg(feature = "dev-tools")]
        probe_readback: Default::default(),
        targets: BTreeSet::new(),
        dense_node: Vec::new(),
        dense_node_local_slot: Vec::new(),
        node_layouts: NodeMap::default(),
        dense_spans: DenseSpans::default(),
        cluster_storage: Vec::new(),
        node_slots: BTreeMap::new(),
        storage_refs: StorageRefs::with_probe_count(0),
        dense_slots: FirstFitRanges::default(),
        sparse_pools: BTreeMap::new(),
        compose_words: Vec::new(),
        sampled_words: Vec::new(),
        installed: BTreeMap::new(),
        sampleable: BTreeSet::new(),
        pending_promotion: BTreeSet::new(),
        pending_depth_tile_texels: BTreeSet::new(),
        dirty_rows: BTreeSet::new(),
        indirect_dirty_rows: BTreeSet::new(),
        indirect_resident_rows: RowBitSet::with_row_capacity(ROW_SPACE),
        indirect_base_row_refs: BTreeMap::from([(2, 1)]),
        indirect_delta_row_refs: BTreeMap::from([(2, 1)]),
        direct_promotion_dirty_rows: BTreeSet::new(),
        direct_animated_dirty_rows: BTreeSet::new(),
        direct_promotion_resident_rows: RowBitSet::with_row_capacity(ROW_SPACE),
        direct_animated_resident_rows: RowBitSet::with_row_capacity(ROW_SPACE),
        direct_base_row_refs: BTreeMap::new(),
        direct_promotion_row_refs: BTreeMap::new(),
        direct_animated_row_refs: BTreeMap::new(),
        direct_required: false,
        direct_compose_required: false,
        animated_direct_compose_required: false,
        direct_animation_descriptor_indices: Vec::new(),
        compose_planner: StreamedComposePlanner::with_row_capacity(ROW_SPACE),
        mover_channel_rows: Default::default(),
        released_mover_cells: Vec::new(),
        mover_pool_bytes: (0, 0),
        compose_frame_plan: None,
        sample_region_index: SampleRegionIndex::build(
            &sample_regions::SampleRegionGrid {
                origin: [0.0; 3],
                cell_size: [1.0; 3],
                dimensions: [4, 4, 4],
                dense_nodes: &[],
            },
            std::iter::empty(),
        )
        .unwrap(),
        compose_input_regions: Vec::new(),
        compose_region_rows: Vec::new(),
        compose_residency_rows: Vec::new(),
        compose_membership_touched: RowQueue::with_row_capacity(ROW_SPACE),
        compose_membership_changes: Vec::new(),
        compose_animated_weights: Vec::new(),
        indirect_compose_diagnostics: ShComposePassDiagnostics::default(),
        static_direct_compose_diagnostics: ShComposePassDiagnostics::default(),
        animated_direct_compose_diagnostics: ShComposePassDiagnostics::default(),
        compose_planning_cpu_micros: 0,
        generation_has_reset: false,
        indirect_compose_epoch: 0,
        direct_compose_epoch: 0,
        install_cpu: InstallCpuCounters::default(),
        pool_bound: Default::default(),
        whole_dense_slots: 0,
        whole_map_refusal_warned: false,
        sparse_compose: Default::default(),
        gpu: None,
    };
    state.indirect_resident_rows = state.rebuilt_resident_rows()[0].iter().copied().collect();
    assert_eq!(state.indirect_resident_rows, BTreeSet::from([2]));
    state
        .release_row_refs(RowRefTable::IndirectBase, 2, 1)
        .unwrap();
    assert_eq!(state.indirect_resident_rows, BTreeSet::from([2]));
    state
        .release_row_refs(RowRefTable::IndirectDelta, 2, 1)
        .unwrap();
    assert!(state.indirect_resident_rows.is_empty());
    state.indirect_dirty_rows.insert(2);
    state.direct_promotion_dirty_rows.insert(4);
    state.direct_animated_dirty_rows.insert(4);
    state.dirty_rows.extend([
        (INDIRECT_DELTA_ID, 2),
        (DIRECT_DELTA_ID, 4),
        (ANIMATED_DIRECT_DELTA_ID, 4),
    ]);
    state.prune_nonresident_dirty_rows();
    assert_eq!(state.snapshot().dirty_affinity_rows, 0);

    // Regression: a fully evicted row was rescanned forever, but a later
    // install must still be able to seed fresh work for the same row.
    state.indirect_delta_row_refs.insert(2, 1);
    state.indirect_resident_rows.insert(2);
    state.indirect_dirty_rows.insert(2);
    state.dirty_rows.insert((INDIRECT_DELTA_ID, 2));
    state.prune_nonresident_dirty_rows();
    assert_eq!(state.snapshot().dirty_affinity_rows, 1);
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
fn ready_batch_over_the_retired_two_install_cap_passes_the_contract() {
    // The controller's decoded-byte budget decides how many clusters a drain
    // carries; small-cluster maps routinely hand over more than two.
    let state = state_with_clusters(3);
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

    state
        .validate_batch_contract(&batch)
        .expect("ready count is install-budget policy, not a boundary rule");
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
            reason: "post-growth sparse pool remains undersized"
        }
        .is_retryable_retirement_pressure()
    );
}

#[test]
fn a_cluster_that_wrote_nothing_adds_no_compose_epoch_of_its_own() {
    // A cluster whose storage is all live publishes no new word or CSR pair.
    // Its own writes must not raise the epoch, or it would wait forever for
    // work that was correctly skipped; any wait comes from pending installs.
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
        referenced_nodes: Vec::new(),
        sparse_rows: Vec::new(),
        required_indirect_epoch,
        required_direct_epoch,
    };

    // The installation drain starts at 41/71. A later drain can sample only
    // after both the indirect (id34/id27) and direct (id35/id41/id45)
    // compose passes have completed their required next epoch.
    assert!(!rows::installed_cluster_is_sampleable(&installed, 41, 71));
    assert!(!rows::installed_cluster_is_sampleable(&installed, 42, 71));
    assert!(!rows::installed_cluster_is_sampleable(&installed, 41, 72));
    assert!(rows::installed_cluster_is_sampleable(&installed, 42, 72));
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
