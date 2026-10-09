//! Reach targeting from the cell-demand stage, decoded-byte install budget,
//! request cancellation, and I/O-facing counters.

use std::collections::BTreeSet;

use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_test_log_capture::LogCapture;
use postretro_visibility::{VisibilityPath, VisibleCells};

use super::tests::{
    controller, controller_with_nominal_budget, hinted_topology, mark_sampleable, prepared,
    topology,
};
use super::*;
use crate::lightmap_streaming::test_fixtures::{PORTAL, residency_set};
use crate::sh_streaming::generation::FixedGenerationClock;
use crate::streaming::cell_demand::CellDemand;
use crate::streaming::drain_budget::MAX_INSTALL_DECODED_BYTES_PER_DRAIN;

const MIB: usize = 1024 * 1024;

fn one_cell_per_cluster(count: usize) -> PlannerTopology {
    topology(
        (0..count as u32).collect(),
        vec![Vec::new(); count],
        vec![Vec::new(); count],
        vec![1; count],
    )
}

/// Six cells, one cluster each. From camera cell 0, cells 1 and 2 lie within
/// the default 16 m lead, 3 (20 m) and 4 (30 m) in the band; 5 is unreached.
fn corridor_set() -> CellResidencySetSection {
    residency_set(
        6,
        &[
            (0, 0, 0),
            (0, 1, 4),
            (0, 2, 12),
            (0, 3, 20),
            (0, 4, 30),
            (1, 1, 0),
            (1, 0, 4),
            (1, 2, 4),
            (1, 3, 10),
            (1, 4, 24),
        ],
        32,
    )
}

fn reach_update(
    controller: &mut ShResidencyController,
    stage: &CellDemand,
    set: &CellResidencySetSection,
    camera_cell: u32,
    path: VisibilityPath,
    visible: &[u32],
    now: f64,
) {
    let visible = VisibleCells::Culled(visible.to_vec());
    controller
        .update_targets(
            &visible,
            Some(stage.frame(set, camera_cell, path, &visible)),
            now,
        )
        .unwrap();
}

fn clusters_of_class(controller: &ShResidencyController, class: TargetClass) -> BTreeSet<u32> {
    controller
        .states
        .iter()
        .enumerate()
        .filter_map(|(cluster_id, state)| (state.class == Some(class)).then_some(cluster_id as u32))
        .collect()
}

fn ids(ids: &[u32]) -> BTreeSet<u32> {
    ids.iter().copied().collect()
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

fn budgeted(topology: PlannerTopology, floor_bytes: u64) -> ShResidencyController {
    ShResidencyController::for_test_with_budget(
        topology,
        &FixedGenerationClock::new(1),
        ShGpuBudgetInputs {
            renderer_effective_floor_bytes: Some(floor_bytes),
            ..ShGpuBudgetInputs::default()
        },
    )
    .unwrap()
}

#[test]
fn sh_mandatory_tier_is_id51_reach_within_lead() {
    // Cells 4 and 5 share cluster 4: cell 4 lies in the band, cell 5 within L,
    // so the cluster takes its nearest cell's lead. Cluster 3 is unreached.
    let mut controller = controller(topology(
        vec![0, 1, 2, 3, 4, 4],
        vec![Vec::new(); 5],
        vec![Vec::new(); 5],
        vec![1; 5],
    ));
    let set = residency_set(
        6,
        &[(0, 0, 0), (0, 1, 8), (0, 2, 20), (0, 4, 25), (0, 5, 10)],
        32,
    );
    let stage = CellDemand::new(set.max_lead);
    reach_update(&mut controller, &stage, &set, 0, PORTAL, &[0], 0.0);

    assert_eq!(
        clusters_of_class(&controller, TargetClass::Visible),
        ids(&[0])
    );
    assert_eq!(
        clusters_of_class(&controller, TargetClass::Lead),
        ids(&[1, 4])
    );
    assert_eq!(clusters_of_class(&controller, TargetClass::Band), ids(&[2]));
    assert!(!controller.is_targeted(3), "outside the reach");
    assert_eq!(
        controller.unsettled_targets(),
        Some(3),
        "the lead tier is in the settle set; the band is not"
    );
    for request in drain_requests(&mut controller) {
        assert_eq!(
            request.mandatory,
            request.cluster_id != 2,
            "cluster {} reads in the mandatory tier only within L",
            request.cluster_id
        );
    }
    for cluster_id in [0, 1, 4] {
        mark_sampleable(&mut controller, cluster_id);
    }
    assert_eq!(
        controller.unsettled_targets(),
        Some(0),
        "settles with the band cluster still cold"
    );
}

#[test]
fn one_lead_from_the_stage_moves_the_sh_split() {
    let mut controller = controller(one_cell_per_cluster(6));
    let set = corridor_set();
    let mut stage = CellDemand::new(set.max_lead);
    reach_update(&mut controller, &stage, &set, 0, PORTAL, &[0], 0.0);
    assert_eq!(
        clusters_of_class(&controller, TargetClass::Lead),
        ids(&[1, 2])
    );
    assert_eq!(
        clusters_of_class(&controller, TargetClass::Band),
        ids(&[3, 4])
    );

    stage.set_lead_metres(32.0);
    reach_update(&mut controller, &stage, &set, 0, PORTAL, &[0], 0.1);
    assert_eq!(
        clusters_of_class(&controller, TargetClass::Lead),
        ids(&[1, 2, 3, 4])
    );
    stage.set_lead_metres(0.0);
    reach_update(&mut controller, &stage, &set, 0, PORTAL, &[0], 0.2);
    assert!(clusters_of_class(&controller, TargetClass::Lead).is_empty());
    assert_eq!(
        clusters_of_class(&controller, TargetClass::Band),
        ids(&[1, 2, 3, 4])
    );
}

#[test]
fn sh_without_id51_targets_no_lead_or_band_tier() {
    let mut controller = controller(hinted_topology(
        vec![0, 1, 2, 3],
        vec![vec![1], vec![0, 2], vec![1, 3], vec![2]],
        vec![vec![], vec![], vec![], vec![2]],
        vec![1; 4],
        Vec::new(),
        [1].into_iter().collect(),
        Vec::new(),
    ));
    controller
        .update_targets(&VisibleCells::Culled(vec![3]), None, 0.0)
        .unwrap();
    assert_eq!(
        controller.targets().clone(),
        ids(&[1, 2, 3]),
        "visible 3, its owner 2, and pin 1: no neighbour, lead or band"
    );
    assert!(clusters_of_class(&controller, TargetClass::Lead).is_empty());
    assert!(clusters_of_class(&controller, TargetClass::Band).is_empty());
    assert_eq!(controller.unsettled_targets(), Some(3));
}

#[test]
fn turning_in_place_keeps_the_reach_and_issues_no_new_reach_requests() {
    let mut controller = controller(one_cell_per_cluster(6));
    let set = corridor_set();
    let stage = CellDemand::new(set.max_lead);
    // The camera stays in cell 0 while the view sweeps across clusters.
    let sweeps = [vec![0], vec![5], vec![4], vec![5], vec![0]];
    for (frame, visible) in sweeps.into_iter().enumerate() {
        reach_update(
            &mut controller,
            &stage,
            &set,
            0,
            PORTAL,
            &visible,
            frame as f64 * 0.1,
        );
        let reach: BTreeSet<_> = clusters_of_class(&controller, TargetClass::Lead)
            .union(&clusters_of_class(&controller, TargetClass::Band))
            .copied()
            .chain(visible.iter().copied().filter(|&cluster| cluster != 5))
            .collect();
        assert_eq!(
            reach,
            ids(&[0, 1, 2, 3, 4]),
            "frame {frame}: the reach follows the camera cell, not the view"
        );
        let issued = stream_until_idle(&mut controller);
        if frame > 0 {
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
fn authored_priority_outranks_reach_lead_in_request_order() {
    let mut controller = controller(hinted_topology(
        (0..6).collect(),
        vec![Vec::new(); 6],
        vec![Vec::new(); 6],
        vec![1; 6],
        Vec::new(),
        Default::default(),
        vec![0, 0, 0, 0, 3, 0],
    ));
    let set = corridor_set();
    let stage = CellDemand::new(set.max_lead);
    reach_update(&mut controller, &stage, &set, 0, PORTAL, &[0], 0.0);
    let requests: Vec<_> = drain_requests(&mut controller)
        .into_iter()
        .map(|request| request.cluster_id)
        .collect();
    // Visible, then lead by lead; in the band, priority 3 before nearer 3.
    assert_eq!(requests, vec![0, 1, 2, 4, 3]);
}

#[test]
fn band_yields_farthest_lead_first_mandatory_never_refused() {
    let set = corridor_set();
    let stage = CellDemand::new(set.max_lead);
    let sized = || {
        topology(
            (0..6).collect(),
            vec![Vec::new(); 6],
            vec![Vec::new(); 6],
            vec![8; 6],
        )
    };

    // Visible 0 and lead 1, 2 take 24 bytes; one band cluster fits beside them.
    let mut controller = budgeted(sized(), 32);
    reach_update(&mut controller, &stage, &set, 0, PORTAL, &[0], 0.0);
    let _ = controller.take_async_drain_batch().unwrap();
    for cluster_id in 0..5 {
        mark_sampleable(&mut controller, cluster_id);
    }
    let batch = controller.take_async_drain_batch().unwrap();
    assert_eq!(
        batch.target_remove,
        vec![4],
        "the farthest band cluster yields"
    );
    assert!(controller.is_targeted(3), "the nearer band cluster stays");

    // Below the mandatory set: the band yields and the mandatory tier stays,
    // past the budget.
    let mut controller = budgeted(sized(), 8);
    reach_update(&mut controller, &stage, &set, 0, PORTAL, &[0], 0.0);
    let _ = controller.take_async_drain_batch().unwrap();
    for cluster_id in 0..5 {
        mark_sampleable(&mut controller, cluster_id);
    }
    let batch = controller.take_async_drain_batch().unwrap();
    assert_eq!(batch.target_remove, vec![3, 4]);
    for cluster_id in [0, 1, 2] {
        assert!(controller.is_targeted(cluster_id), "mandatory {cluster_id}");
    }
    assert!(controller.non_evictable_overshoot_bytes() > 0);
}

#[test]
fn non_portal_path_keeps_sh_visible_and_stage_lead_classes() {
    let mut controller = controller(one_cell_per_cluster(6));
    let set = corridor_set();
    let stage = CellDemand::new(set.max_lead);
    // A frustum fallback draws 5: SH still requests every drawn cluster as
    // Visible, and its reach is the camera cell's baked set.
    reach_update(
        &mut controller,
        &stage,
        &set,
        0,
        VisibilityPath::NoPortalsFallback,
        &[0, 5],
        0.0,
    );
    assert_eq!(
        clusters_of_class(&controller, TargetClass::Visible),
        ids(&[0, 5])
    );
    assert_eq!(
        clusters_of_class(&controller, TargetClass::Lead),
        ids(&[1, 2])
    );
    assert_eq!(
        clusters_of_class(&controller, TargetClass::Band),
        ids(&[3, 4])
    );

    // A solid camera cell with no baked set keeps the reach; cluster 0, no
    // longer drawn, falls back to its lead class.
    reach_update(
        &mut controller,
        &stage,
        &set,
        5,
        VisibilityPath::SolidCellFallback,
        &[5],
        0.1,
    );
    assert_eq!(
        clusters_of_class(&controller, TargetClass::Lead),
        ids(&[0, 1, 2])
    );
    assert_eq!(
        clusters_of_class(&controller, TargetClass::Band),
        ids(&[3, 4])
    );
}

// Regression: the async frame took read requests before its drain's budget
// policy ran, so a target that policy suppressed was submitted while still
// published as a target, and an idle issuer could read it before the republish.
#[test]
fn async_frame_requests_nothing_its_own_budget_policy_suppresses() {
    let set = corridor_set();
    let stage = CellDemand::new(set.max_lead);
    let mut controller = budgeted(
        topology(
            (0..6).collect(),
            vec![Vec::new(); 6],
            vec![Vec::new(); 6],
            vec![8; 6],
        ),
        24,
    );
    reach_update(&mut controller, &stage, &set, 0, PORTAL, &[0], 0.0);
    let _ = controller.take_async_drain_batch().unwrap();
    for cluster_id in 0..4 {
        mark_sampleable(&mut controller, cluster_id);
    }
    // The mandatory set fills the pool; band cluster 4, the farthest, is
    // targeted but has never been requested.
    assert!(controller.is_targeted(4));

    let (batch, requests) = controller.take_async_drain_batch_and_requests().unwrap();
    assert_eq!(batch.target_remove, vec![3, 4]);
    assert!(
        requests.is_empty(),
        "requested {:?} after the drain suppressed it",
        requests
            .iter()
            .map(|request| request.cluster_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(controller.state(4), Some(ClusterResidencyState::Absent));
    assert_eq!(controller.permits_in_use(), 0);
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
        .update_targets(&VisibleCells::Culled(vec![0]), None, 0.0)
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
        .update_targets(&VisibleCells::Culled(vec![0]), None, 2.3)
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
        .update_targets(&VisibleCells::Culled(vec![0]), None, 0.0)
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
fn requests_for_visible_and_pinned_targets_are_mandatory() {
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
        .update_targets(&VisibleCells::Culled(vec![0]), None, 0.0)
        .unwrap();

    let requests: Vec<_> = drain_requests(&mut controller)
        .into_iter()
        .map(|request| (request.cluster_id, request.mandatory))
        .collect();
    assert_eq!(requests, vec![(0, true), (2, true)]);
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
        .update_targets(&VisibleCells::Culled(vec![0, 1]), None, 0.0)
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
        .update_targets(&VisibleCells::Culled(vec![0]), None, 0.1)
        .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), None, 2.2)
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

// R7: the cluster-count warm horizon and its id-46 walk are retired; SH's
// reach is id 51 through the cell-demand stage.
#[test]
fn no_production_path_reads_warm_set() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![src];
    let mut offenders = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy();
            if !name.ends_with(".rs") || name.contains("test") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let production = source.split("#[cfg(test)]").next().unwrap();
            for needle in ["WARM_SET_CLUSTERS", "warm_set", "WarmSource", "warm_rank"] {
                if production.contains(needle) {
                    offenders.push(format!("{}: {needle}", path.display()));
                }
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}
