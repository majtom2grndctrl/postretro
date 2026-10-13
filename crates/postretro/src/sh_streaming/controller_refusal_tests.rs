//! Device-limit outcomes at the renderer/controller seam.

use super::tests::{
    controller, controller_with_nominal_budget, hinted_topology, mark_sampleable, prepared,
    topology,
};
use super::*;
use crate::lightmap_streaming::test_fixtures::{PORTAL, residency_set};
use crate::sh_streaming::generation::FixedGenerationClock;
use crate::sh_streaming::sync_manifest_test_fixture;
use crate::sh_streaming::topology::SeamPortalEndpoint;
use crate::streaming::cell_demand::CellDemand;
use postretro_visibility::VisibleCells;

fn independent(count: usize) -> ShResidencyController {
    controller(topology(
        (0..count as u32).collect(),
        vec![vec![]; count],
        vec![vec![]; count],
        vec![1; count],
    ))
}

fn view(controller: &mut ShResidencyController, visible: &[u32], now: f64) {
    controller
        .update_targets(&VisibleCells::Culled(visible.to_vec()), None, now)
        .unwrap();
}

fn ready_requested(controller: &mut ShResidencyController, cluster_id: u32) {
    assert_eq!(
        controller.state(cluster_id),
        Some(ClusterResidencyState::Queued)
    );
    assert_eq!(
        controller
            .admit_prepared(prepared(controller, cluster_id))
            .unwrap(),
        ShDrainAdmission::Ready
    );
}

fn refuse_batch(controller: &mut ShResidencyController, ids: &[u32]) {
    let batch = controller.take_async_drain_batch().unwrap();
    assert_eq!(
        batch
            .ready
            .iter()
            .map(|p| p.chunk.cluster_id)
            .collect::<Vec<_>>(),
        ids
    );
    assert!(batch.evictions.is_empty());
    controller
        .apply_drain_outcome(ShDrainOutcome {
            refused: ids.to_vec(),
            ..ShDrainOutcome::default()
        })
        .unwrap();
}

fn refusal(controller: &mut ShResidencyController, cluster_id: u32) {
    let request = controller.take_next_request().unwrap().unwrap();
    assert_eq!(request.cluster_id, cluster_id);
    ready_requested(controller, cluster_id);
    refuse_batch(controller, &[cluster_id]);
}

fn leave_and_return(controller: &mut ShResidencyController, id: u32, now: f64) {
    view(controller, &[], now);
    view(controller, &[], now + HYSTERESIS_SECONDS + 0.01);
    assert!(!controller.is_targeted(id));
    view(controller, &[id], now + HYSTERESIS_SECONDS + 0.1);
}

fn assert_ledger(controller: &ShResidencyController) {
    let held = controller
        .states
        .iter()
        .filter(|s| {
            matches!(
                s.state,
                ClusterResidencyState::Queued | ClusterResidencyState::Ready
            )
        })
        .count();
    assert_eq!(controller.permits_in_use(), held);
    let bytes: u64 = controller
        .ready
        .values()
        .map(|r| r.byte_charge)
        .sum::<u64>()
        + controller.in_drain.values().sum::<u64>();
    assert_eq!(controller.accounting().cpu.ready.current_bytes, bytes);
}

fn assert_bucket_total(controller: &ShResidencyController) {
    let counters = controller.counters();
    let b = counters.miss_buckets;
    assert_eq!(
        counters.misses,
        b.outside_reach
            + b.trimmed_by_pressure
            + b.read_in_flight
            + b.held_by_drain_budget
            + b.awaiting_compose
            + b.failed
            + b.device_limited
    );
}

#[test]
fn refusal_releases_ready_charge_and_permit_once_without_changing_other_work() {
    let mut c = independent(4);
    view(&mut c, &[0, 1, 2, 3], 0.0);
    assert_eq!(c.take_requests().unwrap().len(), 4);
    for id in [0, 1, 2] {
        ready_requested(&mut c, id);
    }
    let mut batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.ready.len(), 3);
    let deferred = batch.ready.pop().unwrap();
    c.apply_drain_outcome(ShDrainOutcome {
        accepted: vec![0],
        refused: vec![1],
        deferred: vec![deferred],
        ..ShDrainOutcome::default()
    })
    .unwrap();
    assert_eq!(c.state(0), Some(ClusterResidencyState::InstalledUncomposed));
    assert_eq!(c.state(1), Some(ClusterResidencyState::Failed));
    assert_eq!(c.state(2), Some(ClusterResidencyState::Ready));
    assert_eq!(c.state(3), Some(ClusterResidencyState::Queued));
    assert_eq!(c.permits_in_use(), 2);
    assert_eq!(c.accounting().cpu.ready.current_bytes, 4);
    assert_ledger(&c);
    let before = c.report_snapshot();
    assert!(
        c.apply_drain_outcome(ShDrainOutcome {
            refused: vec![1],
            ..ShDrainOutcome::default()
        })
        .is_err()
    );
    assert_eq!(c.report_snapshot(), before);
}

#[test]
fn refusal_outcome_validation_rejects_all_invalid_partitions_before_mutation() {
    for case in 0..7 {
        let mut c = independent(3);
        view(&mut c, &[0, 1], 0.0);
        mark_sampleable(&mut c, 2);
        c.take_requests().unwrap();
        ready_requested(&mut c, 0);
        ready_requested(&mut c, 1);
        let mut batch = c.take_async_drain_batch().unwrap();
        assert_eq!(batch.evictions, vec![2]);
        let deferred = batch.ready.remove(0);
        let mut outcome = ShDrainOutcome {
            refused: vec![0],
            accepted: vec![1],
            evicted: vec![2],
            ..ShDrainOutcome::default()
        };
        match case {
            0 => outcome.accepted.push(0),
            1 => outcome.dropped.push(0),
            2 => outcome.deferred.push(deferred),
            3 => outcome.refused.push(0),
            4 => outcome.refused.push(99),
            5 => {
                outcome.accepted.clear();
            }
            6 => outcome.refused.push(2),
            _ => unreachable!(),
        }
        let before = c.report_snapshot();
        let in_drain = c.in_drain.clone();
        let evictions = c.in_drain_evictions.clone();
        assert!(
            matches!(
                c.apply_drain_outcome(outcome),
                Err(ShResidencyControllerError::InvalidDrainOutcome(_))
            ),
            "case {case}"
        );
        assert_eq!(c.report_snapshot(), before, "case {case}");
        assert_eq!(c.in_drain, in_drain);
        assert_eq!(c.in_drain_evictions, evictions);
        assert!(c.states[0].failure.is_none());
        assert_ledger(&c);
    }
}

#[test]
fn refusal_accounting_underflow_rejects_the_whole_outcome_before_eviction() {
    let mut c = independent(2);
    view(&mut c, &[0], 0.0);
    mark_sampleable(&mut c, 1);
    let request = c.take_next_request().unwrap().unwrap();
    assert_eq!(request.cluster_id, 0);
    ready_requested(&mut c, 0);
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.evictions, vec![1]);
    c.accounting.cpu.ready.current_bytes = 0;
    let before = c.report_snapshot();
    assert!(matches!(
        c.apply_drain_outcome(ShDrainOutcome {
            evicted: vec![1],
            refused: vec![0],
            ..ShDrainOutcome::default()
        }),
        Err(ShResidencyControllerError::AccountingUnderflow(_))
    ));
    assert_eq!(c.report_snapshot(), before);
    assert_eq!(c.in_drain.len(), 1);
}

#[test]
fn refusal_counts_only_new_visible_episodes_without_rebucketing() {
    let mut c = independent(1);
    view(&mut c, &[0], 0.0);
    let before = c.counters();
    refusal(&mut c, 0);
    view(&mut c, &[0], 0.1);
    assert_eq!(c.counters().misses, before.misses);
    assert_eq!(c.counters().miss_buckets, before.miss_buckets);
    view(&mut c, &[], 0.2);
    view(&mut c, &[0], 0.3);
    assert_eq!(c.counters().miss_buckets.device_limited, 1);
    assert_eq!(c.counters().misses, before.misses + 1);
    assert_bucket_total(&c);
}

#[test]
fn settling_refusal_holds_mandatory_and_reveal_counts_first_device_limit_miss() {
    let mut c = independent(1);
    c.suspend_visible_misses(true);
    view(&mut c, &[0], 0.0);
    refusal(&mut c, 0);
    view(&mut c, &[0], 10.0);
    assert_eq!(c.unsettled_targets(), Some(1));
    assert_eq!(c.counters().misses, 0);
    c.suspend_visible_misses(false);
    view(&mut c, &[0], 10.1);
    assert_eq!(c.counters().miss_buckets.device_limited, 1);
    assert_eq!(c.unsettled_targets(), Some(1));
    assert_bucket_total(&c);
}

#[test]
fn refusal_in_reveal_drain_preserves_the_reveal_frames_existing_miss() {
    let mut c = independent(1);
    c.suspend_visible_misses(true);
    view(&mut c, &[0], 0.0);
    c.take_next_request().unwrap().unwrap();
    ready_requested(&mut c, 0);
    c.suspend_visible_misses(false);
    view(&mut c, &[0], 10.1);
    assert_eq!(c.counters().miss_buckets.held_by_drain_budget, 1);
    refuse_batch(&mut c, &[0]);
    view(&mut c, &[0], 10.2);
    assert_eq!(c.counters().misses, 1);
    assert_eq!(c.counters().miss_buckets.held_by_drain_budget, 1);
    assert_eq!(c.counters().miss_buckets.device_limited, 0);
    assert_bucket_total(&c);
}

#[test]
fn refused_optional_seam_target_does_not_hold_settling() {
    let mut c = controller(hinted_topology(
        vec![0, 1],
        vec![vec![]; 2],
        vec![vec![]; 2],
        vec![1; 2],
        vec![SeamPortalEndpoint {
            portal_id: 0,
            front_cluster_id: 0,
            back_cluster_id: 1,
        }],
        BTreeSet::new(),
        vec![],
    ));
    c.suspend_visible_misses(true);
    view(&mut c, &[0], 0.0);
    mark_sampleable(&mut c, 0);
    refusal(&mut c, 1);
    assert_eq!(c.unsettled_targets(), Some(0));
    assert_eq!(c.counters().misses, 0);
}

#[test]
fn refusal_retries_on_repeated_expired_leave_and_reentry_without_spending_read_retry() {
    let mut c = independent(1);
    view(&mut c, &[0], 0.0);
    refusal(&mut c, 0);
    view(&mut c, &[], 0.1);
    view(&mut c, &[0], 0.2);
    assert!(c.take_next_request().unwrap().is_none());
    for cycle in 0..4 {
        leave_and_return(&mut c, 0, 3.0 + 3.0 * f64::from(cycle));
        assert_eq!(c.state(0), Some(ClusterResidencyState::Absent));
        refusal(&mut c, 0);
        assert!(!c.states[0].failure.unwrap().retry_spent);
        assert_ledger(&c);
    }
    assert_eq!(c.counters().retries, 4);
}

#[test]
fn refusal_read_retry_sequences_preserve_the_lifetime_read_budget_and_warning() {
    // Both orderings cross the loader's real hash check and controller's async failure seam.
    for initial_read_failure in [false, true] {
        let (_temp, path) = sync_manifest_test_fixture::write_one_cluster_prl();
        let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
        let manifest = Arc::clone(world.sh_stream_manifest().unwrap());
        let hints = Arc::new(ClusterHints::decode(manifest.cluster_directory()).unwrap());
        let mut c = ShResidencyController::with_clock(
            Arc::clone(&manifest),
            ShGpuBudgetInputs::default(),
            hints,
            &FixedGenerationClock::new(1),
        )
        .unwrap();
        let good_bytes = manifest.read_and_decode_cluster(0).unwrap().bytes;
        let mut bad_bytes = good_bytes.clone();
        bad_bytes[0] ^= 1;
        let fail_hash = |c: &mut ShResidencyController, should_warn| {
            let request = c.take_next_request().unwrap().unwrap();
            let error = manifest
                .decode_encoded_cluster(0, bad_bytes.clone())
                .unwrap_err();
            assert!(error.to_string().to_lowercase().contains("hash"), "{error}");
            assert_eq!(c.admit_failed_request(request).unwrap(), should_warn);
            assert_ledger(c);
        };
        view(&mut c, &[0], 0.0);
        let mut now = 0.1;
        if initial_read_failure {
            fail_hash(&mut c, true);
            leave_and_return(&mut c, 0, now);
            now += 3.0;
        }
        refusal(&mut c, 0);
        leave_and_return(&mut c, 0, now);
        now += 3.0;
        fail_hash(&mut c, !initial_read_failure);
        leave_and_return(&mut c, 0, now);
        now += 3.0;
        if initial_read_failure {
            assert!(c.take_next_request().unwrap().is_none());
        } else {
            // Spend the read retry, then exercise another refusal before the final failed read.
            refusal(&mut c, 0);
            leave_and_return(&mut c, 0, now);
            now += 3.0;
            fail_hash(&mut c, false);
            leave_and_return(&mut c, 0, now);
            assert!(c.take_next_request().unwrap().is_none());
        }
        assert!(c.states[0].failure.unwrap().retry_spent);
        assert!(c.states[0].failure.unwrap().read_failed);
    }
}

#[test]
fn refused_owner_blocks_all_dependent_reads_bytes_and_permits_until_retry_is_sampleable() {
    let mut c = controller(topology(
        vec![0, 1, 2],
        vec![vec![]; 3],
        vec![vec![2], vec![2], vec![]],
        vec![1; 3],
    ));
    view(&mut c, &[0, 1], 0.0);
    refusal(&mut c, 2);
    for now in [0.1, 1.0, 3.0] {
        view(&mut c, &[0, 1], now);
        assert!(c.take_requests().unwrap().is_empty());
        for id in [0, 1] {
            assert_eq!(c.state(id), Some(ClusterResidencyState::Absent));
        }
        assert_eq!(c.permits_in_use(), 0);
        assert_eq!(c.accounting().cpu.ready.current_bytes, 0);
    }
    view(&mut c, &[], 3.1);
    view(&mut c, &[], 5.11);
    view(&mut c, &[0, 1], 5.2);
    let request = c.take_next_request().unwrap().unwrap();
    assert_eq!(request.cluster_id, 2);
    ready_requested(&mut c, 2);
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.ready.len(), 1);
    c.apply_drain_outcome(ShDrainOutcome {
        accepted: vec![2],
        ..ShDrainOutcome::default()
    })
    .unwrap();
    assert!(c.take_requests().unwrap().is_empty());
    c.promote_composed_clusters();
    assert_eq!(c.take_requests().unwrap().len(), 2);
    assert_ledger(&c);
}

#[test]
fn device_limit_owner_dominates_read_failure_anywhere_in_closure_in_either_order() {
    for owners in [vec![1, 2], vec![2, 1]] {
        let mut c = controller(topology(
            vec![0, 1, 2, 3],
            vec![vec![]; 4],
            vec![owners, vec![], vec![3], vec![]],
            vec![1; 4],
        ));
        // Prepare both failure causes while not presenting a frame.
        c.suspend_visible_misses(true);
        view(&mut c, &[0], 0.0);
        let requests = c.take_requests().unwrap();
        assert_eq!(
            requests
                .iter()
                .map(|r| r.cluster_id)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([1, 3])
        );
        assert!(
            c.admit_failed_request(*requests.iter().find(|r| r.cluster_id == 1).unwrap())
                .unwrap()
        );
        ready_requested(&mut c, 3);
        refuse_batch(&mut c, &[3]);
        c.suspend_visible_misses(false);
        view(&mut c, &[0], 0.1);
        assert_eq!(c.counters().miss_buckets.device_limited, 1);
        assert_eq!(c.counters().miss_buckets.failed, 0);
        assert_eq!(c.state(0), Some(ClusterResidencyState::Absent));
        assert_eq!(c.state(2), Some(ClusterResidencyState::Absent));
        assert_eq!(c.permits_in_use(), 0);
        assert_bucket_total(&c);
    }
}

#[test]
fn later_eviction_does_not_wake_targeted_refusal_and_refusal_does_not_request_evictions() {
    let mut c = independent(2);
    view(&mut c, &[0, 1], 0.0);
    mark_sampleable(&mut c, 1);
    refusal(&mut c, 0);
    view(&mut c, &[0], 0.1);
    view(&mut c, &[0], 2.2);
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.evictions, vec![1]);
    c.apply_drain_outcome(ShDrainOutcome {
        evicted: vec![1],
        ..ShDrainOutcome::default()
    })
    .unwrap();
    view(&mut c, &[0], 3.0);
    assert!(c.take_requests().unwrap().is_empty());
    assert_eq!(c.state(0), Some(ClusterResidencyState::Failed));
    assert!(c.take_async_drain_batch().unwrap().evictions.is_empty());
}

#[test]
fn refusal_does_not_evict_pin_lead_owner_or_optional_residents_when_floor_covers_targets() {
    let mut c = controller_with_nominal_budget(
        hinted_topology(
            vec![0, 1, 2, 3, 4, 5],
            vec![vec![]; 6],
            vec![vec![3], vec![], vec![], vec![], vec![], vec![]],
            vec![1; 6],
            vec![],
            BTreeSet::from([1]),
            vec![],
        ),
        6,
    );
    let set = residency_set(6, &[(0, 2, 4), (0, 4, 24)], 32);
    let stage = CellDemand::new(set.max_lead);
    let visible = VisibleCells::Culled(vec![0, 5]);
    c.update_targets(&visible, Some(stage.frame(&set, 0, PORTAL, &visible)), 0.0)
        .unwrap();
    for id in [0, 1, 2, 3, 4] {
        mark_sampleable(&mut c, id);
    }
    refusal(&mut c, 5);
    for now in [0.1, 2.5, 20.0] {
        c.update_targets(&visible, Some(stage.frame(&set, 0, PORTAL, &visible)), now)
            .unwrap();
        let batch = c.take_async_drain_batch().unwrap();
        assert!(batch.evictions.is_empty());
        assert!(batch.ready.is_empty());
        c.apply_drain_outcome(ShDrainOutcome::default()).unwrap();
        for id in [1, 2, 3, 4] {
            assert_eq!(c.state(id), Some(ClusterResidencyState::Sampleable));
        }
        assert_eq!(c.non_evictable_overshoot_bytes(), 0);
    }
}

#[test]
fn optional_refusal_is_skipped_only_by_trim_and_does_not_inflate_protected_overshoot() {
    let mut c = controller_with_nominal_budget(
        hinted_topology(
            vec![0, 1],
            vec![vec![]; 2],
            vec![vec![]; 2],
            vec![8; 2],
            vec![SeamPortalEndpoint {
                portal_id: 0,
                front_cluster_id: 0,
                back_cluster_id: 1,
            }],
            BTreeSet::new(),
            vec![],
        ),
        8,
    );
    view(&mut c, &[0], 0.0);
    // Read optional 1 while 0 is in flight, so the initial floor permits its install attempt.
    let requests = c.take_requests().unwrap();
    assert_eq!(requests.len(), 2);
    ready_requested(&mut c, 1);
    refuse_batch(&mut c, &[1]);
    ready_requested(&mut c, 0);
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.ready.len(), 1);
    c.apply_drain_outcome(ShDrainOutcome {
        accepted: vec![0],
        ..ShDrainOutcome::default()
    })
    .unwrap();
    c.promote_composed_clusters();
    // Lower the policy floor to force trimming even though the refused optional has no occupancy.
    c.accounting = ShResidencyAccounting::new(ShGpuBudgetInputs {
        renderer_effective_floor_bytes: Some(4),
        ..ShGpuBudgetInputs::default()
    })
    .unwrap();
    c.accounting.add_logical(8).unwrap();
    let batch = c.take_async_drain_batch().unwrap();
    assert!(batch.evictions.is_empty());
    assert!(c.is_targeted(1));
    assert!(!c.states[1].suppressed);
    assert_eq!(
        c.non_evictable_overshoot_bytes(),
        4,
        "only visible cluster 0 contributes"
    );
}

#[test]
fn early_seam_reentry_never_wakes_on_time_alone_and_later_reentry_does() {
    let mut c = controller(hinted_topology(
        vec![0, 1, 2],
        vec![vec![]; 3],
        vec![vec![]; 3],
        vec![1; 3],
        vec![SeamPortalEndpoint {
            portal_id: 0,
            front_cluster_id: 1,
            back_cluster_id: 0,
        }],
        BTreeSet::new(),
        vec![],
    ));
    // Both background camera cells start warm in a real target set. Turning
    // away retains the far one through ordinary horizon hysteresis.
    view(&mut c, &[1, 2], 0.0);
    for id in [1, 2] {
        mark_sampleable(&mut c, id);
    }
    view(&mut c, &[1], 0.0);
    refusal(&mut c, 0);
    view(&mut c, &[2], 0.1);
    assert!(
        !c.is_targeted(0),
        "seam-only work has no horizon hysteresis"
    );
    view(&mut c, &[1], 0.2);
    assert!(c.is_targeted(0));
    for now in [0.5, 2.0, 3.0] {
        view(&mut c, &[1], now);
        assert!(c.take_requests().unwrap().is_empty());
    }
    view(&mut c, &[2], 3.1);
    view(&mut c, &[1], 3.2);
    assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 0);
    assert_eq!(c.counters().retries, 1);
}

#[test]
fn rapid_seam_flicker_reads_refused_cluster_at_most_once_per_hysteresis_window() {
    let mut c = controller(hinted_topology(
        vec![0, 1, 2],
        vec![vec![]; 3],
        vec![vec![]; 3],
        vec![1; 3],
        vec![SeamPortalEndpoint {
            portal_id: 0,
            front_cluster_id: 1,
            back_cluster_id: 0,
        }],
        BTreeSet::new(),
        vec![],
    ));
    // Both background camera cells start warm in a real target set. Turning
    // away retains the far one through ordinary horizon hysteresis.
    view(&mut c, &[1, 2], 0.0);
    for id in [1, 2] {
        mark_sampleable(&mut c, id);
    }
    view(&mut c, &[1], 0.0);
    refusal(&mut c, 0);
    let mut refusal_times = vec![0.0];
    for tick in 1..=400 {
        let now = f64::from(tick) / 60.0;
        let near = if tick % 2 == 0 { 1 } else { 2 };
        view(&mut c, &[near], now);
        if let Some(request) = c.take_next_request().unwrap() {
            assert_eq!(request.cluster_id, 0);
            assert!(now - refusal_times.last().unwrap() >= HYSTERESIS_SECONDS);
            ready_requested(&mut c, 0);
            refuse_batch(&mut c, &[0]);
            refusal_times.push(now);
        } else {
            let batch = c.take_async_drain_batch().unwrap();
            assert!(batch.evictions.is_empty());
            c.apply_drain_outcome(ShDrainOutcome::default()).unwrap();
        }
        assert_ledger(&c);
    }
    assert!(
        refusal_times.len() >= 3,
        "the seam retries once its cooldown has elapsed"
    );
}

#[test]
fn rapid_horizon_churn_under_pressure_keeps_targeted_refusal_failed() {
    let mut c = controller_with_nominal_budget(
        topology(vec![0, 1, 2], vec![vec![]; 3], vec![vec![]; 3], vec![8; 3]),
        24,
    );
    view(&mut c, &[1, 2], 0.0);
    for id in [1, 2] {
        mark_sampleable(&mut c, id);
    }
    view(&mut c, &[0, 1], 0.0);
    refusal(&mut c, 0);
    c.accounting = ShResidencyAccounting::new(ShGpuBudgetInputs {
        renderer_effective_floor_bytes: Some(4),
        ..ShGpuBudgetInputs::default()
    })
    .unwrap();
    c.accounting.add_logical(16).unwrap();
    for tick in 1..=400 {
        let now = f64::from(tick) / 60.0;
        view(&mut c, &[0, if tick % 2 == 0 { 1 } else { 2 }], now);
        let batch = c.take_async_drain_batch().unwrap();
        assert!(batch.evictions.is_empty());
        c.apply_drain_outcome(ShDrainOutcome::default()).unwrap();
        assert!(c.take_requests().unwrap().is_empty());
        assert_eq!(c.state(0), Some(ClusterResidencyState::Failed));
        assert_ledger(&c);
    }
    assert_eq!(c.counters().retries, 0);
}

#[test]
fn pressure_suppression_of_dependent_cannot_churn_its_refused_owner() {
    let mut c = controller_with_nominal_budget(
        topology(
            vec![0, 1, 2, 3],
            vec![vec![]; 4],
            vec![vec![], vec![0], vec![], vec![]],
            vec![8; 4],
        ),
        32,
    );
    view(&mut c, &[2, 3], 0.0);
    for id in [2, 3] {
        mark_sampleable(&mut c, id);
    }
    let set = residency_set(4, &[(2, 1, 24), (3, 1, 24)], 32);
    let stage = CellDemand::new(set.max_lead);
    let update = |c: &mut ShResidencyController, camera, now| {
        let visible = VisibleCells::Culled(vec![camera]);
        c.update_targets(
            &visible,
            Some(stage.frame(&set, camera, PORTAL, &visible)),
            now,
        )
        .unwrap();
    };
    update(&mut c, 2, 0.0);
    refusal(&mut c, 0);
    c.accounting = ShResidencyAccounting::new(ShGpuBudgetInputs {
        renderer_effective_floor_bytes: Some(4),
        ..ShGpuBudgetInputs::default()
    })
    .unwrap();
    c.accounting.add_logical(16).unwrap();
    let mut last_retry = 0.0;
    for tick in 1..=400 {
        let now = f64::from(tick) / 60.0;
        // Each pair has a stable horizon frame in which suppression removes the owner,
        // followed by a changed horizon that can restore the dependent and owner.
        let camera = if (tick / 2) % 2 == 0 { 2 } else { 3 };
        let before = c.counters().retries;
        update(&mut c, camera, now);
        if c.counters().retries > before {
            assert!(now - last_retry >= HYSTERESIS_SECONDS);
            last_retry = now;
        }
        let batch = c.take_async_drain_batch().unwrap();
        assert!(batch.evictions.is_empty());
        c.apply_drain_outcome(ShDrainOutcome::default()).unwrap();
        // A refused owner cannot acquire a dependent permit. Once a retry becomes
        // Absent the ordinary trim may suppress it too; it still does no I/O here.
        assert!(c.take_requests().unwrap().is_empty());
        assert_eq!(c.state(1), Some(ClusterResidencyState::Absent));
        assert_eq!(c.permits_in_use(), 0);
        assert_eq!(c.accounting().cpu.ready.current_bytes, 0);
        if c.state(0) == Some(ClusterResidencyState::Failed) && c.is_targeted(0) {
            assert!(!c.states[0].suppressed, "trim must skip the refused owner");
        }
    }
}

#[test]
fn newly_visible_dependent_counts_refused_owner_before_it_enters_the_target_set() {
    let mut c = controller(topology(
        vec![0, 1],
        vec![vec![]; 2],
        vec![vec![1], vec![]],
        vec![1; 2],
    ));
    c.suspend_visible_misses(true);
    view(&mut c, &[1], 0.0);
    refusal(&mut c, 1);
    assert!(!c.is_targeted(0));
    c.suspend_visible_misses(false);
    view(&mut c, &[0], 0.1);
    assert_eq!(c.counters().miss_buckets.device_limited, 1);
    assert_eq!(c.counters().miss_buckets.outside_reach, 0);
    assert!(c.take_requests().unwrap().is_empty());
    assert_bucket_total(&c);
}

#[test]
fn ordinary_pressure_eviction_in_refusal_drain_does_not_trigger_further_eviction() {
    let mut c = controller_with_nominal_budget(
        topology(vec![0, 1, 2], vec![vec![]; 3], vec![vec![]; 3], vec![8; 3]),
        16,
    );
    let set = residency_set(3, &[(2, 1, 24)], 32);
    let stage = CellDemand::new(set.max_lead);
    let visible = VisibleCells::Culled(vec![0, 2]);
    c.update_targets(&visible, Some(stage.frame(&set, 2, PORTAL, &visible)), 0.0)
        .unwrap();
    for id in [1, 2] {
        mark_sampleable(&mut c, id);
    }
    assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 0);
    ready_requested(&mut c, 0);
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(
        batch.evictions,
        vec![1],
        "ordinary pressure sees Ready demand before refusal"
    );
    assert_eq!(batch.ready.len(), 1);
    c.apply_drain_outcome(ShDrainOutcome {
        evicted: vec![1],
        refused: vec![0],
        ..ShDrainOutcome::default()
    })
    .unwrap();
    let later = c.take_async_drain_batch().unwrap();
    assert!(later.evictions.is_empty());
    assert!(later.ready.is_empty());
    assert_eq!(c.state(2), Some(ClusterResidencyState::Sampleable));
    assert_eq!(c.accounting().logical_occupancy_bytes, 8);
    assert_ledger(&c);
}

#[test]
fn lifting_owner_suppression_without_a_horizon_change_does_not_retry_refusal() {
    let mut c = controller_with_nominal_budget(
        topology(
            vec![0, 1, 2],
            vec![vec![]; 3],
            vec![vec![], vec![0], vec![]],
            vec![8; 3],
        ),
        32,
    );
    mark_sampleable(&mut c, 2);
    let set = residency_set(3, &[(2, 1, 24)], 32);
    let stage = CellDemand::new(set.max_lead);
    let visible = VisibleCells::Culled(vec![2]);
    let update = |c: &mut ShResidencyController, now| {
        c.update_targets(&visible, Some(stage.frame(&set, 2, PORTAL, &visible)), now)
            .unwrap();
    };
    update(&mut c, 0.0);
    refusal(&mut c, 0);
    c.update_gpu_charges(FixedGpuCharges {
        fixed_metadata_bytes: 28,
        ..FixedGpuCharges::default()
    })
    .unwrap();
    let batch = c.take_async_drain_batch().unwrap();
    assert!(batch.evictions.is_empty());
    c.apply_drain_outcome(ShDrainOutcome::default()).unwrap();
    update(&mut c, 0.1);
    assert!(!c.is_targeted(0));
    c.update_gpu_charges(FixedGpuCharges::default()).unwrap();
    update(&mut c, 3.0);
    assert!(c.is_targeted(0));
    assert!(c.is_targeted(1));
    assert_eq!(c.state(0), Some(ClusterResidencyState::Failed));
    assert!(c.take_requests().unwrap().is_empty());
    assert_eq!(c.counters().retries, 0);
}
