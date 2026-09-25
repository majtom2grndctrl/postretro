//! Warm-set targeting, decoded-byte install budget, request cancellation, and
//! I/O-facing counters.

use std::collections::BTreeSet;

use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_loader::CoupledCellPair;
use postretro_test_log_capture::LogCapture;
use postretro_visibility::VisibleCells;

use super::tests::{
    controller, controller_with_nominal_budget, hinted_topology, mark_sampleable, prepared,
    topology,
};
use super::*;
use crate::sh_streaming::generation::FixedGenerationClock;

const METRE: u32 = 1024;
const MIB: usize = 1024 * 1024;

fn pair(cell_a: usize, cell_b: usize, distance: u32, aperture: u32) -> CoupledCellPair {
    CoupledCellPair {
        cell_a,
        cell_b,
        distance,
        aperture,
    }
}

/// Cells `0..count` in a line, one metre apart, all one metre wide.
fn chain_pairs(count: usize) -> Vec<CoupledCellPair> {
    (1..count)
        .map(|cell| pair(cell - 1, cell, METRE, METRE))
        .collect()
}

fn one_cell_per_cluster(count: usize) -> PlannerTopology {
    topology(
        (0..count as u32).collect(),
        vec![Vec::new(); count],
        vec![Vec::new(); count],
        vec![1; count],
    )
}

fn warm_controller(topology: PlannerTopology, pairs: &[CoupledCellPair]) -> ShResidencyController {
    ShResidencyController::for_test_with_cell_pairs(topology, pairs, ShGpuBudgetInputs::default())
        .unwrap()
}

/// Warm clusters nearest first.
fn warm_order(controller: &ShResidencyController) -> Vec<u32> {
    let mut clusters: Vec<_> = controller.warm.clusters().collect();
    clusters.sort_by_key(|&cluster_id| (controller.warm.rank(cluster_id), cluster_id));
    clusters
}

fn clusters_of_class(controller: &ShResidencyController, class: TargetClass) -> BTreeSet<u32> {
    controller
        .states
        .iter()
        .enumerate()
        .filter_map(|(cluster_id, state)| (state.class == Some(class)).then_some(cluster_id as u32))
        .collect()
}

fn drain_requests(controller: &mut ShResidencyController) -> Vec<ShClusterRequest> {
    std::iter::from_fn(|| controller.take_next_request().unwrap()).collect()
}

/// Requests, reads, installs, and composes until no target needs a request.
/// Returns each request with the class it carried when issued.
fn stream_until_idle(controller: &mut ShResidencyController) -> Vec<(u32, TargetClass)> {
    let mut issued = Vec::new();
    loop {
        let requests = drain_requests(controller);
        if requests.is_empty() {
            return issued;
        }
        for request in requests {
            let class = controller.states[request.cluster_id as usize]
                .class
                .expect("a requested cluster is targeted");
            issued.push((request.cluster_id, class));
            let admission = controller
                .admit_prepared(prepared(controller, request.cluster_id))
                .unwrap();
            assert_eq!(admission, ShDrainAdmission::Ready);
        }
        let batch = controller.take_async_drain_batch().unwrap();
        let accepted = batch
            .ready
            .iter()
            .map(|prepared| prepared.chunk.cluster_id)
            .collect();
        controller
            .apply_drain_outcome(ShDrainOutcome {
                accepted,
                ..ShDrainOutcome::default()
            })
            .unwrap();
        controller.promote_composed_clusters();
    }
}

#[test]
fn turning_in_place_keeps_the_prefetch_set_and_issues_no_new_prefetch_requests() {
    let mut controller = warm_controller(one_cell_per_cluster(12), &chain_pairs(12));
    let warm: BTreeSet<u32> = (0..WARM_SET_CLUSTERS as u32).collect();
    // The camera stays in cell 0 while the view sweeps across disjoint far
    // clusters, then returns to the first. The old visible-seeded two-hop
    // horizon moved with every one of these frames.
    let sweeps = [vec![8], vec![9], vec![10], vec![11], vec![8]];
    for (frame, visible) in sweeps.into_iter().enumerate() {
        controller
            .update_targets(&VisibleCells::Culled(visible), Some(0), frame as f64 * 0.1)
            .unwrap();
        assert_eq!(
            clusters_of_class(&controller, TargetClass::Prefetch),
            warm,
            "frame {frame}: prefetch follows the camera cell, not the view"
        );
        let issued = stream_until_idle(&mut controller);
        if frame == 0 {
            let prefetched: BTreeSet<_> = issued
                .iter()
                .filter_map(|&(cluster_id, class)| {
                    (class == TargetClass::Prefetch).then_some(cluster_id)
                })
                .collect();
            assert_eq!(prefetched, warm);
        } else {
            assert!(
                issued
                    .iter()
                    .all(|&(_, class)| class == TargetClass::Visible),
                "frame {frame} issued non-visible requests: {issued:?}"
            );
        }
    }
}

#[test]
fn warm_walk_reaches_clusters_beyond_the_camera_cells_own_pairs() {
    // Cells 0, 1, and 2 form the camera cluster. Every pair stored for the
    // camera cell stays inside it; only composed pairs leave it.
    let pairs = [
        pair(0, 1, METRE, METRE),
        pair(0, 2, METRE, METRE),
        pair(1, 3, METRE, METRE),
        pair(3, 4, METRE, METRE),
    ];
    let cell_to_cluster = vec![0, 0, 0, 1, 2];
    assert!(
        pairs
            .iter()
            .filter(|pair| pair.cell_a == 0 || pair.cell_b == 0)
            .all(|pair| cell_to_cluster[pair.cell_a] == 0 && cell_to_cluster[pair.cell_b] == 0)
    );
    let mut controller = warm_controller(
        topology(
            cell_to_cluster,
            vec![Vec::new(); 3],
            vec![Vec::new(); 3],
            vec![1; 3],
        ),
        &pairs,
    );

    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();

    assert_eq!(warm_order(&controller), vec![0, 1, 2]);
    assert_eq!(
        clusters_of_class(&controller, TargetClass::Prefetch),
        BTreeSet::from([1, 2])
    );
}

#[test]
fn warm_set_is_bounded_includes_the_camera_cluster_and_breaks_ties_by_cell() {
    let mut controller = warm_controller(one_cell_per_cluster(20), &chain_pairs(20));

    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), Some(10), 0.0)
        .unwrap();

    let warm: BTreeSet<_> = controller.warm.clusters().collect();
    assert_eq!(warm.len(), WARM_SET_CLUSTERS);
    assert!(warm.contains(&10), "the camera's own cluster is warm");
    // 9/11, 8/12, 7/13 sit at 1, 2, and 3 m. Cells 6 and 14 tie at 4 m with
    // equal aperture; the lower cell settles first and fills the last slot.
    assert_eq!(warm, (6..=13).collect());
    assert_eq!(controller.targets, warm);
    assert_eq!(
        controller.report_snapshot().warm_clusters,
        WARM_SET_CLUSTERS
    );
}

#[test]
fn warm_walk_stops_after_the_settled_cell_cap() {
    // One cluster owns exactly the cap's worth of cells; the next cluster is
    // one metre past the last of them.
    let cell_count = WARM_WALK_MAX_SETTLED_CELLS + 1;
    let mut cell_to_cluster = vec![0; WARM_WALK_MAX_SETTLED_CELLS];
    cell_to_cluster.push(1);
    let mut controller = warm_controller(
        topology(
            cell_to_cluster,
            vec![Vec::new(); 2],
            vec![Vec::new(); 2],
            vec![1; 2],
        ),
        &chain_pairs(cell_count),
    );

    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), Some(0), 0.0)
        .unwrap();

    assert_eq!(warm_order(&controller), vec![0]);
}

#[test]
fn warm_walk_is_independent_of_pair_order() {
    let mut pairs = chain_pairs(12);
    pairs.push(pair(0, 5, 3 * METRE, 2 * METRE));
    let mut reversed = pairs.clone();
    reversed.reverse();
    let ranked = |pairs: &[CoupledCellPair]| {
        let mut controller = warm_controller(one_cell_per_cluster(12), pairs);
        controller
            .update_targets(&VisibleCells::Culled(Vec::new()), Some(2), 0.0)
            .unwrap();
        warm_order(&controller)
    };

    assert_eq!(ranked(&pairs), ranked(&reversed));
}

/// Camera cell 0 is cluster 0. Other cells map to clusters so that id order
/// disagrees with rank order at every step.
fn ranking_fixture(authored_priorities: Vec<u32>) -> ShResidencyController {
    let pairs = [
        pair(0, 4, METRE, METRE),    // cluster 4: 1 m, aperture 1 m
        pair(0, 2, 2253, 5 * METRE), // cluster 2: 2.2 m, aperture 5 m
        pair(0, 3, 2970, 5 * METRE), // cluster 1: 2.9 m, aperture 5 m
        pair(0, 1, 2560, METRE),     // cluster 3: 2.5 m, aperture 1 m
        pair(4, 5, 1229, 9 * METRE), // cluster 5: 2.2 m, bottleneck 1 m
    ];
    warm_controller(
        hinted_topology(
            vec![0, 3, 2, 1, 4, 5],
            vec![Vec::new(); 6],
            vec![Vec::new(); 6],
            vec![1; 6],
            Vec::new(),
            Default::default(),
            authored_priorities,
        ),
        &pairs,
    )
}

#[test]
fn warm_ranking_follows_distance_bucket_then_aperture_then_cluster_id() {
    let mut controller = ranking_fixture(Vec::new());
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();

    // Bucket 0: cluster 0. Bucket 1: cluster 4. Bucket 2: clusters 1 and 2
    // share the widest aperture and order by id; clusters 3 and 5 share the
    // one-metre aperture (5 through its bottleneck) and order by id.
    assert_eq!(warm_order(&controller), vec![0, 4, 1, 2, 3, 5]);
    let requests: Vec<_> = drain_requests(&mut controller)
        .into_iter()
        .map(|request| request.cluster_id)
        .collect();
    assert_eq!(requests, vec![0, 4, 1, 2, 3, 5]);
}

#[test]
fn authored_priority_outranks_warm_distance_in_request_order() {
    let mut controller = ranking_fixture(vec![0, 0, 0, 0, 0, 3]);
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();

    let requests: Vec<_> = drain_requests(&mut controller)
        .into_iter()
        .map(|request| request.cluster_id)
        .collect();
    assert_eq!(requests, vec![0, 5, 4, 1, 2, 3]);
}

#[test]
fn pressure_yields_the_farthest_equal_priority_prefetch_first() {
    let mut controller = ShResidencyController::for_test_with_cell_pairs(
        topology(
            vec![0, 1, 2, 3],
            vec![Vec::new(); 4],
            vec![Vec::new(); 4],
            vec![8; 4],
        ),
        &chain_pairs(4),
        ShGpuBudgetInputs {
            renderer_effective_floor_bytes: Some(16),
            ..ShGpuBudgetInputs::default()
        },
    )
    .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let _ = controller.take_async_drain_batch().unwrap();
    for cluster_id in 0..4 {
        mark_sampleable(&mut controller, cluster_id);
    }

    // Only the visible cluster and one prefetch fit. The LRU/id tie-break
    // alone would have released cluster 1, the nearest.
    let batch = controller.take_async_drain_batch().unwrap();
    assert_eq!(batch.target_remove, vec![2, 3]);
    assert!(controller.is_targeted(1));
    assert!(controller.states[2].suppressed && controller.states[3].suppressed);
}

// Regression: the async frame took read requests before its drain's budget
// policy ran, so a prefetch that policy suppressed was submitted while still
// published as a target, and an idle issuer could read it before the republish.
#[test]
fn async_frame_requests_nothing_its_own_budget_policy_suppresses() {
    let mut controller = ShResidencyController::for_test_with_cell_pairs(
        topology(
            vec![0, 1, 2, 3],
            vec![Vec::new(); 4],
            vec![Vec::new(); 4],
            vec![8; 4],
        ),
        &chain_pairs(4),
        ShGpuBudgetInputs {
            renderer_effective_floor_bytes: Some(16),
            ..ShGpuBudgetInputs::default()
        },
    )
    .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let _ = controller.take_async_drain_batch().unwrap();
    for cluster_id in 0..3 {
        mark_sampleable(&mut controller, cluster_id);
    }
    // Three resident clusters already exceed the two-cluster pool; cluster 3,
    // the farthest prefetch, is targeted but has never been requested.
    assert!(controller.is_targeted(3));

    let (batch, requests) = controller.take_async_drain_batch_and_requests().unwrap();
    assert_eq!(batch.target_remove, vec![2, 3]);
    assert!(
        requests.is_empty(),
        "requested {:?} after the drain suppressed it",
        requests
            .iter()
            .map(|request| request.cluster_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(controller.state(3), Some(ClusterResidencyState::Absent));
    assert_eq!(controller.permits_in_use(), 0);
}

#[test]
fn missing_cell_visibility_falls_back_to_two_hops_from_the_camera_cluster_and_warns_once() {
    let capture = LogCapture::start();
    let mut controller = controller(topology(
        vec![0, 1, 2, 3, 4],
        vec![vec![1], vec![0, 2], vec![1, 3], vec![2, 4], vec![3]],
        vec![Vec::new(); 5],
        vec![1; 5],
    ));

    // The far cluster is visible, but prefetch expands from the camera.
    controller
        .update_targets(&VisibleCells::Culled(vec![4]), Some(0), 0.0)
        .unwrap();
    assert_eq!(controller.targets(), &BTreeSet::from([0, 1, 2, 4]));
    controller
        .update_targets(&VisibleCells::Culled(vec![4]), Some(1), 0.1)
        .unwrap();
    assert_eq!(warm_order(&controller), vec![0, 1, 2, 3]);
    controller
        .update_targets(&VisibleCells::Culled(vec![4]), None, 0.2)
        .unwrap();
    assert_eq!(controller.warm.len(), 0, "no camera cell has no warm set");

    capture.assert_logged_once(
        log::Level::Warn,
        "CellVisibility (id 46) is absent; prefetch falls back to 2-hop cluster adjacency",
    );
}

#[test]
fn cell_visibility_that_disagrees_with_the_cell_map_falls_back_and_warns() {
    let capture = LogCapture::start();
    let topology = topology(
        vec![0, 1, 2],
        vec![vec![1], vec![0, 2], vec![1]],
        vec![Vec::new(); 3],
        vec![1; 3],
    );
    let warm_source = WarmSource::resolve(Some((2, &chain_pairs(2))), 3);
    let mut controller = ShResidencyController::from_parts(
        None,
        topology,
        warm_source,
        &FixedGenerationClock::new(1),
        ShGpuBudgetInputs::default(),
    )
    .unwrap();

    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), Some(0), 0.0)
        .unwrap();

    assert_eq!(warm_order(&controller), vec![0, 1, 2]);
    capture.assert_logged_once(
        log::Level::Warn,
        "CellVisibility (id 46) has 2 cells but the id-49 cell map has 3",
    );
}

fn ready_with_bytes(controller: &mut ShResidencyController, cluster_id: u32, len: usize) {
    let request = controller.take_next_request().unwrap().unwrap();
    assert_eq!(request.cluster_id, cluster_id);
    let admission = controller
        .admit_prepared(PreparedShCluster {
            generation: controller.generation(),
            content_tag: controller.content_tag(),
            chunk: DecodedClusterShPayload {
                cluster_id,
                bytes: vec![0; len],
                blocks: Vec::new(),
            },
        })
        .unwrap();
    assert_eq!(admission, ShDrainAdmission::Ready);
}

/// Every cluster visible, so ready work installs in cluster-id order.
fn budget_controller(sizes: &[usize]) -> ShResidencyController {
    let mut controller = controller(one_cell_per_cluster(sizes.len()));
    controller
        .update_targets(&VisibleCells::DrawAll, None, 0.0)
        .unwrap();
    for (cluster_id, &len) in sizes.iter().enumerate() {
        ready_with_bytes(&mut controller, cluster_id as u32, len);
    }
    controller
}

fn take_batch(controller: &mut ShResidencyController, async_path: bool) -> Vec<u32> {
    let batch = if async_path {
        controller.take_async_drain_batch()
    } else {
        controller.take_drain_batch()
    }
    .unwrap();
    let ids: Vec<_> = batch
        .ready
        .iter()
        .map(|prepared| prepared.chunk.cluster_id)
        .collect();
    controller
        .apply_drain_outcome(ShDrainOutcome {
            accepted: ids.clone(),
            ..ShDrainOutcome::default()
        })
        .unwrap();
    ids
}

#[test]
fn byte_budget_installs_one_oversized_cluster_alone() {
    for async_path in [false, true] {
        let mut controller = budget_controller(&[9 * MIB, 4]);

        assert_eq!(take_batch(&mut controller, async_path), vec![0]);
        let counters = controller.counters();
        assert_eq!(counters.budget_limited_drains, 1);
        assert_eq!(counters.decoded_bytes_installed, 9 * MIB as u64);
        assert_eq!(counters.last_drain_decoded_bytes, 9 * MIB as u64);
        assert_eq!(counters.max_drain_decoded_bytes, 9 * MIB as u64);

        assert_eq!(take_batch(&mut controller, async_path), vec![1]);
        let counters = controller.counters();
        assert_eq!(counters.budget_limited_drains, 1);
        assert_eq!(counters.decoded_bytes_installed, 9 * MIB as u64 + 4);
        assert_eq!(counters.last_drain_decoded_bytes, 4);
        assert_eq!(counters.max_drain_decoded_bytes, 9 * MIB as u64);

        assert!(take_batch(&mut controller, async_path).is_empty());
        assert_eq!(
            controller.counters().last_drain_decoded_bytes,
            4,
            "an empty drain keeps the last installing drain's figure"
        );
    }
}

#[test]
fn byte_budget_fills_up_to_the_limit_with_small_clusters() {
    for async_path in [false, true] {
        let mut controller = budget_controller(&[2 * MIB; 5]);

        assert_eq!(take_batch(&mut controller, async_path), vec![0, 1, 2, 3]);
        assert_eq!(
            controller.counters().last_drain_decoded_bytes,
            MAX_INSTALL_DECODED_BYTES_PER_DRAIN
        );
        assert_eq!(controller.counters().budget_limited_drains, 1);
        assert_eq!(take_batch(&mut controller, async_path), vec![4]);
        assert_eq!(controller.counters().budget_limited_drains, 1);
    }
}

#[test]
fn byte_budget_stops_rather_than_skipping_to_smaller_work() {
    for async_path in [false, true] {
        // Cluster 2 would fit beside cluster 0, but it ranks after cluster 1.
        let mut controller = budget_controller(&[5 * MIB, 4 * MIB, MIB]);

        assert_eq!(take_batch(&mut controller, async_path), vec![0]);
        assert_eq!(controller.counters().budget_limited_drains, 1);
        assert_eq!(take_batch(&mut controller, async_path), vec![1, 2]);
        assert_eq!(controller.counters().budget_limited_drains, 1);
        assert_eq!(
            controller.counters().decoded_bytes_installed,
            10 * MIB as u64
        );
    }
}

#[test]
fn cancelled_request_on_a_retargeted_cluster_returns_to_absent_without_failure() {
    let mut controller = controller(topology(vec![0], vec![vec![]], vec![vec![]], vec![1]));
    let capture = LogCapture::start();
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let request = controller.take_next_request().unwrap().unwrap();

    // The cluster leaves, outlives hysteresis, and is targeted again while
    // its request is still queued at the issuer.
    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 0.1)
        .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 2.2)
        .unwrap();
    assert!(!controller.is_targeted(0));
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 2.3)
        .unwrap();
    assert!(controller.is_targeted(0));
    assert_eq!(controller.state(0), Some(ClusterResidencyState::Queued));

    controller.admit_cancelled_request(request).unwrap();
    assert_eq!(controller.state(0), Some(ClusterResidencyState::Absent));
    assert_eq!(controller.permits_in_use(), 0);
    assert_eq!(controller.counters().cancelled_requests, 1);

    // A repeated completion is a no-op once the request is no longer queued.
    controller.admit_cancelled_request(request).unwrap();
    assert_eq!(controller.counters().cancelled_requests, 1);
    assert_eq!(controller.permits_in_use(), 0);

    let reissued = controller.take_next_request().unwrap().unwrap();
    assert_eq!(reissued, request);
    assert!(
        capture
            .records()
            .iter()
            .all(|record| record.level != log::Level::Warn),
        "cancellation never warns"
    );
}

#[test]
fn cancelled_request_with_a_foreign_identity_is_ignored() {
    let mut controller = controller(topology(vec![0], vec![vec![]], vec![vec![]], vec![1]));
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let request = controller.take_next_request().unwrap().unwrap();
    let mut foreign = request;
    foreign.generation += 1;

    controller.admit_cancelled_request(foreign).unwrap();

    assert_eq!(controller.state(0), Some(ClusterResidencyState::Queued));
    assert_eq!(controller.permits_in_use(), 1);
    assert_eq!(controller.counters().cancelled_requests, 0);
}

#[test]
fn requests_are_mandatory_only_for_visible_and_pinned_classes() {
    let mut controller = controller(hinted_topology(
        vec![0, 1, 2],
        vec![vec![1], vec![0], Vec::new()],
        vec![Vec::new(); 3],
        vec![1; 3],
        Vec::new(),
        BTreeSet::from([2]),
        Vec::new(),
    ));
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();

    let requests: Vec<_> = drain_requests(&mut controller)
        .into_iter()
        .map(|request| (request.cluster_id, request.mandatory))
        .collect();
    assert_eq!(requests, vec![(0, true), (2, true), (1, false)]);
}

#[test]
fn admission_counts_every_discarded_completed_read() {
    let mut controller = controller_with_nominal_budget(
        topology(
            vec![0, 1],
            vec![vec![], vec![]],
            vec![vec![], vec![]],
            vec![1, 1],
        ),
        64,
    );
    controller
        .update_targets(&VisibleCells::Culled(vec![0, 1]), Some(0), 0.0)
        .unwrap();
    let first = controller.take_next_request().unwrap().unwrap();
    let second = controller.take_next_request().unwrap().unwrap();

    let mut stale = prepared(&controller, first.cluster_id);
    stale.generation += 1;
    assert_eq!(
        controller.admit_prepared(stale).unwrap(),
        ShDrainAdmission::DroppedStale
    );
    assert_eq!(
        controller
            .admit_prepared(prepared(&controller, first.cluster_id))
            .unwrap(),
        ShDrainAdmission::Ready
    );
    assert_eq!(
        controller
            .admit_prepared(prepared(&controller, first.cluster_id))
            .unwrap(),
        ShDrainAdmission::DroppedDuplicate
    );
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.1)
        .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 2.2)
        .unwrap();
    assert_eq!(
        controller
            .admit_prepared(prepared(&controller, second.cluster_id))
            .unwrap(),
        ShDrainAdmission::DroppedNotTargeted
    );

    let counters = controller.counters();
    assert_eq!(counters.discarded_reads, 3);
    // Three discarded reads of 7 encoded bytes each, not their decoded size.
    assert_eq!(counters.discarded_read_bytes, 21);
}
