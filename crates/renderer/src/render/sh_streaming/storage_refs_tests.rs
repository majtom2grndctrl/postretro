//! Reference-counted shared storage, driven through the real install,
//! eviction and promotion paths on synthetic maps.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use super::install_tests::{SyntheticMap, corrupt_last_probe_patch, mirror, two_cluster_map};
use super::*;

/// Bricks 0–1 are cluster 0's and bricks 2–3 cluster 1's. Cluster 1 also names
/// brick 1, so that brick's node, 64 probe words and sparse rows are shared.
const SHARED_ROW: u32 = 1;
const SECTIONS: [u32; 3] = [INDIRECT_DELTA_ID, DIRECT_DELTA_ID, ANIMATED_DIRECT_DELTA_ID];

fn shared_map() -> SyntheticMap {
    two_cluster_map().with_halo_brick(1, SHARED_ROW)
}

fn shared_node(map: &SyntheticMap, state: &ShResidencyState) -> StoredNode {
    state.dense_node[shared_probes(map)[0] as usize].unwrap()
}

/// The probes both clusters name.
fn shared_probes(map: &SyntheticMap) -> Vec<u32> {
    let other: BTreeSet<u32> = map.cluster_probes(1).into_iter().collect();
    map.cluster_probes(0)
        .into_iter()
        .filter(|dense| other.contains(dense))
        .collect()
}

fn install(state: &mut ShResidencyState, map: &SyntheticMap, cluster_id: u32) {
    let prepared = map.prepared(state, cluster_id);
    state.install(None, &prepared).unwrap();
}

fn evict(state: &mut ShResidencyState, cluster_id: u32) {
    state
        .evict(&mut StagedUploads::default(), cluster_id)
        .unwrap();
}

/// The CPU half of `drain`, in its order: targets, promotion and eviction,
/// then installs. `build` reads the state the batch was prepared against.
fn drain(
    state: &mut ShResidencyState,
    build: impl FnOnce(&ShResidencyState) -> ShDrainBatch,
) -> ShDrainOutcome {
    let batch = build(state);
    state.validate_batch_contract(&batch).unwrap();
    let (new_generation, targets) = state.plan_target_transition(&batch).unwrap();
    assert!(!new_generation, "these drains stay in one generation");
    state.targets = targets;
    let mut outcome = ShDrainOutcome::default();
    state
        .promote_and_evict(&mut StagedUploads::default(), batch.evictions, &mut outcome)
        .unwrap();
    state
        .install_ready(None, batch.ready, PoolAdmission::default(), &mut outcome)
        .unwrap();
    outcome
}

fn batch(state: &ShResidencyState) -> ShDrainBatch {
    ShDrainBatch {
        generation: state.generation,
        content_tag: state.content_tag,
        ..ShDrainBatch::default()
    }
}

/// Everything compose would have finished by the next drain.
fn compose(state: &mut ShResidencyState) {
    state.indirect_compose_epoch += 1;
    state.direct_compose_epoch += 1;
    state.indirect_dirty_rows.clear();
    state.direct_promotion_dirty_rows.clear();
    state.direct_animated_dirty_rows.clear();
    state.dirty_rows.clear();
}

fn promote(state: &mut ShResidencyState) {
    state
        .promote_completed(&mut StagedUploads::default())
        .unwrap();
}

fn assert_nothing_held(state: &ShResidencyState) {
    assert!(state.storage_refs.is_empty());
    assert!(state.node_slots.is_empty());
    assert!(state.installed.is_empty());
    assert!(state.compose_words.iter().all(|&word| word == 0));
    assert!(state.sampled_words.iter().all(|&word| word == 0));
    for section_id in SECTIONS {
        let pool = &state.sparse_pools[&section_id];
        assert_eq!(pool.logical_occupancy(), (0, 0));
        assert!(pool.row_pairs.iter().all(|&pair| pair == [0, 0]));
    }
    assert!(state.indirect_base_row_refs.is_empty());
    assert!(state.indirect_delta_row_refs.is_empty());
    assert_eq!(
        state.resident_rows(),
        [BTreeSet::new(), BTreeSet::new(), BTreeSet::new()]
    );
}

#[test]
fn shared_node_survives_either_eviction_and_frees_with_the_last() {
    for (first, second) in [(0, 1), (1, 0)] {
        let map = shared_map();
        let mut state = map.state();
        install(&mut state, &map, 0);
        install(&mut state, &map, 1);
        let node = shared_node(&map, &state);
        let slots = state.node_slots[&node];
        let high_water = state.dense_slots.capacity;
        assert_eq!(state.storage_refs.node_refs(&node), 2);

        evict(&mut state, first);
        assert_eq!(state.node_slots[&node], slots, "evicted {first} first");
        assert_eq!(state.storage_refs.node_refs(&node), 1);

        evict(&mut state, second);
        assert!(!state.node_slots.contains_key(&node));
        assert_nothing_held(&state);

        // Freed slots are reusable: a full reinstall fits the old high water.
        install(&mut state, &map, second);
        install(&mut state, &map, first);
        assert_eq!(state.dense_slots.capacity, high_water);
        assert_eq!(state.storage_refs.node_refs(&node), 2);
    }
}

#[test]
fn both_clusters_of_a_shared_node_can_leave_in_one_batch() {
    for evictions in [vec![0, 1], vec![1, 0]] {
        let map = shared_map();
        let mut state = map.state();
        install(&mut state, &map, 0);
        install(&mut state, &map, 1);
        let outcome = drain(&mut state, |state| ShDrainBatch {
            target_remove: vec![0, 1],
            evictions,
            ..batch(state)
        });
        assert_eq!(outcome.evicted, [0, 1]);
        assert_nothing_held(&state);
    }
}

#[test]
fn an_eviction_request_for_a_cluster_still_targeted_frees_nothing() {
    let map = shared_map();
    let mut state = map.state();
    install(&mut state, &map, 0);
    install(&mut state, &map, 1);
    let before = mirror(&state);
    let outcome = drain(&mut state, |state| ShDrainBatch {
        evictions: vec![0],
        ..batch(state)
    });
    assert!(outcome.evicted.is_empty());
    assert_eq!(mirror(&state), before);
}

#[test]
fn two_clusters_one_drain_write_a_shared_node_once() {
    let map = shared_map();
    let mut state = map.state();
    let (first, second) = (map.prepared(&state, 0), map.prepared(&state, 1));
    let tiles = |state: &ShResidencyState, cluster_id| {
        map.cluster_probes(cluster_id)
            .iter()
            .map(|&dense| state.dense_node[dense as usize].unwrap())
            .collect::<BTreeSet<_>>()
            .iter()
            .map(|node| state.node_layouts[node].tile_count)
            .sum::<u32>()
    };
    let whole_first = tiles(&state, 0);
    let whole_second = tiles(&state, 1);

    let mut staged_writes = Vec::new();
    for prepared in [&first, &second] {
        let mut journal = InstallJournal::default();
        let staged = state.stage_install(&mut journal, prepared, true).unwrap();
        staged_writes.push((
            staged.local_slots.iter().map(|run| run.len).sum::<u32>(),
            staged.sparse_plan.len(),
        ));
        state.commit_install(prepared.chunk.cluster_id, staged);
    }

    // Each chunk carries the shared brick; only the first uploads it.
    let shared_tiles = state.node_layouts[&shared_node(&map, &state)].tile_count;
    assert_eq!(staged_writes[0].0, whole_first);
    assert_eq!(staged_writes[1].0, whole_second - shared_tiles);
    let row_families = SECTIONS.len();
    assert_eq!(staged_writes[0].1, 2 * row_families);
    assert_eq!(staged_writes[1].1, 2 * row_families);
    // Two nodes plus three, one in common: the pool holds four.
    assert_eq!(state.node_slots.len(), 4);
    assert_eq!(state.storage_refs.node_refs(&shared_node(&map, &state)), 2);

    // The drain path lands on the same mirrors as the staged walk above.
    let mut drained = map.state();
    let outcome = drain(&mut drained, |state| ShDrainBatch {
        ready: vec![map.prepared(state, 0), map.prepared(state, 1)],
        ..batch(state)
    });
    assert_eq!(outcome.accepted, [0, 1]);
    assert_eq!(mirror(&drained), mirror(&state));
}

#[test]
fn a_second_cluster_writes_only_its_own_words_into_a_shared_node() {
    // Probes 0..4 sit in brick 0 but are named by cluster 1 alone, so brick
    // 0's node is shared while no word is.
    let map = two_cluster_map().with_moved_probes(0..4, 1);
    let mut state = map.state();
    install(&mut state, &map, 0);
    let node = state.dense_node[0].unwrap();
    assert_eq!(state.compose_words[0], 0);
    install(&mut state, &map, 1);

    assert_eq!(state.storage_refs.node_refs(&node), 2);
    assert_ne!(state.compose_words[0], 0);
    assert_eq!(state.storage_refs.word_refs(0), 1);
    let tile_of = |state: &ShResidencyState, dense: u32| {
        state.compose_words[dense as usize] >> PROBE_INDIRECTION_SLOT_SHIFT
    };
    let slots = state.node_slots[&node];
    assert!((slots.start..slots.start + slots.len).contains(&tile_of(&state, 0)));

    // Cluster 1's words leave with it; the node stays for cluster 0.
    evict(&mut state, 1);
    assert_eq!(state.compose_words[0], 0);
    assert_eq!(state.node_slots[&node], slots);
    assert_ne!(state.compose_words[map.cluster_probes(0)[0] as usize], 0);
}

#[test]
fn last_reference_clears_before_reuse_and_live_reference_clears_nothing() {
    let map = shared_map();
    let mut state = map.state();
    install(&mut state, &map, 0);
    install(&mut state, &map, 1);
    compose(&mut state);
    promote(&mut state);
    let shared = shared_probes(&map);
    let shared_words: Vec<u32> = shared
        .iter()
        .map(|&dense| state.compose_words[dense as usize])
        .collect();
    assert!(shared_words.iter().all(|&word| word != 0));
    let pairs: Vec<[u32; 2]> = SECTIONS
        .iter()
        .map(|section_id| state.sparse_pools[section_id].row_pairs[SHARED_ROW as usize])
        .collect();
    let node = shared_node(&map, &state);
    let slots = state.node_slots[&node];

    // A live reference: the shared brick keeps every word, pair and slot,
    // and nothing asks compose to look at it again.
    evict(&mut state, 0);
    for (&dense, &word) in shared.iter().zip(&shared_words) {
        assert_eq!(state.compose_words[dense as usize], word);
        assert_eq!(state.sampled_words[dense as usize], word);
    }
    for (section_id, pair) in SECTIONS.iter().zip(&pairs) {
        assert_eq!(
            state.sparse_pools[section_id].row_pairs[SHARED_ROW as usize],
            *pair
        );
        assert!(!state.dirty_rows.contains(&(*section_id, SHARED_ROW)));
    }
    assert!(!state.indirect_dirty_rows.contains(&SHARED_ROW));
    assert_eq!(state.node_slots[&node], slots);

    // The last reference, with a cluster taking those slots in the same
    // drain: words and pair are gone before the install addresses them.
    let outcome = drain(&mut state, |state| ShDrainBatch {
        target_remove: vec![1],
        evictions: vec![1],
        ready: vec![map.prepared(state, 0)],
        ..batch(state)
    });
    assert_eq!(outcome.evicted, [1]);
    assert_eq!(outcome.accepted, [0]);
    // Cluster 0 re-wrote the shared brick into a slot it now holds alone, and
    // the sampled mirror stays a miss until that compose lands.
    assert_eq!(state.storage_refs.node_refs(&node), 1);
    for &dense in &shared {
        assert_ne!(state.compose_words[dense as usize], 0);
        assert_eq!(state.sampled_words[dense as usize], 0);
        assert_eq!(state.storage_refs.word_refs(dense), 1);
    }
    for section_id in SECTIONS {
        assert!(state.dirty_rows.contains(&(section_id, SHARED_ROW)));
    }

    evict(&mut state, 0);
    assert_nothing_held(&state);
}

#[test]
fn halo_cluster_samples_alone() {
    // Cluster 1 names a brick homed on cluster 0, which today holds it back
    // until cluster 0 is sampleable.
    let map = shared_map();
    let mut state = map.state();
    let outcome = drain(&mut state, |state| ShDrainBatch {
        ready: vec![map.prepared(state, 1)],
        ..batch(state)
    });
    assert_eq!(outcome.accepted, [1]);
    assert!(outcome.deferred.is_empty());

    compose(&mut state);
    promote(&mut state);
    assert_eq!(state.installed.keys().copied().collect::<Vec<_>>(), [1]);
    assert_eq!(state.sampleable, BTreeSet::from([1]));
    for dense in map.cluster_probes(1) {
        assert_ne!(state.sampled_words[dense as usize], 0, "probe {dense}");
    }
    for section_id in SECTIONS {
        assert_ne!(
            state.sparse_pools[&section_id].row_pairs[SHARED_ROW as usize],
            [0, 0]
        );
    }
}

/// Install cluster 0, then fail cluster 1's install partway. Cluster 1 shares
/// one node and writes the others first, so the failure must drop a live
/// count back to one and free a node entirely.
fn assert_failure_restores_every_count(
    label: &str,
    map: &SyntheticMap,
    inject: impl FnOnce(&mut ShResidencyState, &mut PreparedShCluster),
    expected: ShResidencyDrainError,
) {
    let mut state = map.state();
    install(&mut state, map, 0);
    let mut prepared = map.prepared(&state, 1);
    inject(&mut state, &mut prepared);
    let node = shared_node(map, &state);
    let first_written = map
        .cluster_probes(1)
        .iter()
        .map(|&dense| state.dense_node[dense as usize].unwrap())
        .find(|node| !state.node_slots.contains_key(node))
        .expect("cluster 1 writes a node of its own");
    let before = mirror(&state);

    assert_eq!(state.install(None, &prepared), Err(expected), "{label}");

    assert_eq!(mirror(&state), before, "{label}");
    assert_eq!(state.storage_refs.node_refs(&node), 1, "{label}");
    assert_eq!(state.storage_refs.node_refs(&first_written), 0, "{label}");
    assert!(!state.node_slots.contains_key(&first_written), "{label}");
}

#[test]
fn failed_install_restores_every_reference_count() {
    let map = shared_map();

    // After every node is referenced, including first-written ones, while the
    // closure is checked.
    assert_failure_restores_every_count(
        "closure check",
        &map,
        |_, prepared| {
            let block = prepared
                .chunk
                .blocks
                .iter()
                .find(|block| block.kind == PROBE_PATCH_BLOCK)
                .unwrap()
                .clone();
            // Move the first patch's rank past the atlas.
            let word = block.body.start + 4;
            prepared.chunk.bytes[word + 3] = 0x7f;
        },
        malformed(1, "probe patch closure rank exceeds id-34 isolated atlas"),
    );

    // After nodes and every earlier word reference.
    assert_failure_restores_every_count(
        "last probe patch",
        &map,
        |_, prepared| corrupt_last_probe_patch(prepared),
        malformed(
            1,
            "probe patch flags disagree with retained id-34 node metadata",
        ),
    );

    // After nodes, words, row refs, and the id-27 and id-41 sparse references:
    // a stale live row refuses the chunk's last id-45 row.
    assert_failure_restores_every_count(
        "sparse allocation",
        &map,
        |state, _| {
            let row = *map.cluster_sparse_rows(1).last().unwrap();
            state
                .sparse_pools
                .get_mut(&ANIMATED_DIRECT_DELTA_ID)
                .unwrap()
                .install(row, 1, 2)
                .unwrap();
        },
        malformed(1, "sparse row cannot allocate pool range"),
    );
}

#[test]
fn a_fully_staged_install_rolls_back_as_a_failed_upload_does() {
    // Upload staging is the last step that can fail, and it holds the same
    // journal: nothing may survive even with every reference taken.
    let map = shared_map();
    let mut state = map.state();
    install(&mut state, &map, 0);
    let before = mirror(&state);
    let prepared = map.prepared(&state, 1);
    let mut journal = InstallJournal::default();
    state.stage_install(&mut journal, &prepared, true).unwrap();
    assert_ne!(mirror(&state), before);
    state.roll_back(journal);
    assert_eq!(mirror(&state), before);
}

#[test]
fn shared_light_accumulates_once_in_either_order() {
    let occupancy = |order: [u32; 2]| {
        let map = shared_map();
        let mut state = map.state();
        for cluster_id in order {
            install(&mut state, &map, cluster_id);
        }
        // Row refs count stored rows, so a shared light's row counts once in
        // the table compose reads, however many clusters name it.
        assert_eq!(state.indirect_delta_row_refs[&SHARED_ROW], 1);
        assert_eq!(state.direct_promotion_row_refs[&SHARED_ROW], 1);
        assert_eq!(state.direct_animated_row_refs[&SHARED_ROW], 1);
        assert_eq!(
            state
                .storage_refs
                .sparse_row_refs(&(INDIRECT_DELTA_ID, SHARED_ROW)),
            2
        );
        SECTIONS.map(|section_id| state.sparse_pools[&section_id].logical_occupancy())
    };
    let forward = occupancy([0, 1]);
    assert_eq!(forward, occupancy([1, 0]));
    // Four bricks, one entry of two halves each, in every family.
    assert_eq!(forward, [(4, 8); 3]);
}

#[test]
fn evict_and_install_one_drain_keep_one_live_copy() {
    for (leaving, arriving) in [(0, 1), (1, 0)] {
        let map = shared_map();
        let mut state = map.state();
        state.targets = BTreeSet::from([leaving]);
        install(&mut state, &map, leaving);
        compose(&mut state);
        promote(&mut state);
        let node = shared_node(&map, &state);

        let outcome = drain(&mut state, |state| ShDrainBatch {
            target_add: vec![arriving],
            target_remove: vec![leaving],
            evictions: vec![leaving],
            ready: vec![map.prepared(state, arriving)],
            ..batch(state)
        });
        assert_eq!(outcome.evicted, [leaving]);
        assert_eq!(outcome.accepted, [arriving]);

        assert_eq!(state.storage_refs.node_refs(&node), 1);
        assert!(state.node_slots.contains_key(&node));
        for section_id in SECTIONS {
            assert_eq!(
                state
                    .storage_refs
                    .sparse_row_refs(&(section_id, SHARED_ROW)),
                1
            );
        }
        // Every word the arriving cluster names points into its live node,
        // and promotion publishes it.
        compose(&mut state);
        promote(&mut state);
        for dense in map.cluster_probes(arriving) {
            let word = state.sampled_words[dense as usize];
            assert_ne!(word, 0, "probe {dense} has a stale zero word");
            let live = state.node_slots[&state.dense_node[dense as usize].unwrap()];
            let slot = word >> PROBE_INDIRECTION_SLOT_SHIFT;
            assert!((live.start..live.start + live.len).contains(&slot));
        }
    }
}

#[test]
fn evict_and_rerequest_counts_references_once() {
    let map = shared_map();

    // Re-requested before the eviction lands: the request is moot, and the
    // chunk that raced it is dropped rather than counted again.
    let mut state = map.state();
    install(&mut state, &map, 0);
    let before = mirror(&state);
    let outcome = drain(&mut state, |state| ShDrainBatch {
        evictions: vec![0],
        ready: vec![map.prepared(state, 0)],
        ..batch(state)
    });
    assert!(outcome.evicted.is_empty());
    assert_eq!(outcome.dropped, [0]);
    assert_eq!(mirror(&state), before);

    // Evicted, its slots re-tenanted by another cluster, then back.
    let mut state = map.state();
    state.targets = BTreeSet::from([0]);
    install(&mut state, &map, 0);
    let node = shared_node(&map, &state);
    drain(&mut state, |state| ShDrainBatch {
        target_add: vec![1],
        target_remove: vec![0],
        evictions: vec![0],
        ready: vec![map.prepared(state, 1)],
        ..batch(state)
    });
    assert_eq!(state.installed.keys().copied().collect::<Vec<_>>(), [1]);
    let outcome = drain(&mut state, |state| ShDrainBatch {
        target_add: vec![0],
        ready: vec![map.prepared(state, 0)],
        ..batch(state)
    });
    assert_eq!(outcome.accepted, [0]);
    assert_eq!(state.storage_refs.node_refs(&node), 2);
    for dense in shared_probes(&map) {
        assert_eq!(state.storage_refs.word_refs(dense), 2);
    }
    evict(&mut state, 0);
    evict(&mut state, 1);
    assert_nothing_held(&state);
}

#[test]
fn read_finishing_after_eviction_is_dropped() {
    let map = shared_map();
    let mut state = map.state();
    state.targets = BTreeSet::from([0, 1]);
    install(&mut state, &map, 1);
    let in_flight = map.prepared(&state, 0);
    let before = mirror(&state);

    // Cluster 0 left the targets while its read was running.
    let outcome = drain(&mut state, |state| ShDrainBatch {
        target_remove: vec![0],
        ready: vec![in_flight],
        ..batch(state)
    });
    assert_eq!(outcome.dropped, [0]);
    assert!(outcome.accepted.is_empty());
    assert_eq!(mirror(&state), before);
}

#[test]
fn shared_reference_waits_for_first_writers_compose() {
    // Cluster 0 names every brick, so cluster 1, installing second, writes
    // nothing and its own epochs alone would let it promote at once.
    let map = two_cluster_map()
        .with_halo_brick(0, 2)
        .with_halo_brick(0, 3);
    let mut state = map.state();
    let outcome = drain(&mut state, |state| ShDrainBatch {
        ready: vec![map.prepared(state, 0), map.prepared(state, 1)],
        ..batch(state)
    });
    assert_eq!(outcome.accepted, [0, 1]);
    assert_eq!(state.installed[&1].required_indirect_epoch, 1);
    assert_eq!(state.installed[&1].required_direct_epoch, 1);

    // The next drain, before compose: neither may publish uncomposed words.
    promote(&mut state);
    assert!(state.sampleable.is_empty());
    assert!(state.sampled_words.iter().all(|&word| word == 0));

    compose(&mut state);
    promote(&mut state);
    assert_eq!(state.sampleable, BTreeSet::from([0, 1]));

    // With the writer already composed, a later sharer owes no wait.
    evict(&mut state, 1);
    install(&mut state, &map, 1);
    assert_eq!(state.installed[&1].required_indirect_epoch, 1);
    promote(&mut state);
    assert!(state.sampleable.contains(&1));
}

#[test]
fn shared_wait_outlives_a_writer_that_leaves_with_rows_still_dirty() {
    let map = shared_map();
    let mut state = map.state();
    install(&mut state, &map, 0);
    install(&mut state, &map, 1);
    // The shared brick is still uncomposed and cluster 1 now holds it alone.
    evict(&mut state, 0);
    assert!(state.indirect_dirty_rows.contains(&SHARED_ROW));
    promote(&mut state);
    assert!(state.sampleable.is_empty());
    compose(&mut state);
    promote(&mut state);
    assert_eq!(state.sampleable, BTreeSet::from([1]));
}

#[test]
fn shared_wait_ends_when_its_writer_leaves_with_the_last_dirty_row() {
    // Cluster 1 names only brick 1, which cluster 2 also names and has already
    // composed. It waits on cluster 0's unrelated write. Cluster 0 then leaves
    // before compose runs, so no pass will ever reach the epoch it asked for.
    let map = SyntheticMap::new([16, 4, 4], [1, 1, 1]).with_halo_brick(2, 1);
    let mut state = map.state();
    install(&mut state, &map, 2);
    compose(&mut state);
    promote(&mut state);
    install(&mut state, &map, 0);
    install(&mut state, &map, 1);
    assert_eq!(state.installed[&1].required_indirect_epoch, 2);
    assert_eq!(state.installed[&1].required_direct_epoch, 2);
    promote(&mut state);
    assert!(!state.sampleable.contains(&1));

    evict(&mut state, 0);
    promote(&mut state);
    assert!(state.sampleable.contains(&1));
}

// Regression: the wait was released only when the evicted cluster was pending,
// so a promoted cluster taking the last dirty row left the wait hanging, and
// the outcome turned on eviction order. Cluster 2 writes nothing, cluster 3
// holding its brick, and waits on cluster 0's writes. Clusters 0 and 1 swap
// sparse rows, so each keeps a dirty row of the other resident.
#[test]
fn shared_wait_ends_whichever_cluster_takes_the_last_dirty_row() {
    for (first, second) in [(0, 1), (1, 0)] {
        let map = SyntheticMap::new([16, 4, 4], [1, 1, 1])
            .with_sparse_row_home(0, 1)
            .with_sparse_row_home(1, 0)
            .with_halo_brick(3, 2);
        let mut state = map.state();
        for cluster_id in [3, 1] {
            install(&mut state, &map, cluster_id);
            compose(&mut state);
            promote(&mut state);
        }
        install(&mut state, &map, 0);
        install(&mut state, &map, 2);
        assert!(state.installed[&2].required_indirect_epoch > state.indirect_compose_epoch);

        evict(&mut state, first);
        promote(&mut state);
        assert!(!state.sampleable.contains(&2), "{first} first: rows dirty");

        evict(&mut state, second);
        promote(&mut state);
        assert!(state.sampleable.contains(&2), "{first} first: nothing left");
    }
}
