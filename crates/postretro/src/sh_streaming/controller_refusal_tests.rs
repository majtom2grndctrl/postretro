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
    assert_eq!(controller.request_owners.len(), held);
    for &cluster_id in controller.request_owners.keys() {
        assert!(matches!(
            controller.states[cluster_id as usize].state,
            ClusterResidencyState::Queued | ClusterResidencyState::Ready
        ));
    }
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

fn install_ready(controller: &mut ShResidencyController, expected: &[u32]) {
    let batch = controller.take_async_drain_batch().unwrap();
    let accepted: Vec<_> = batch.ready.iter().map(|p| p.chunk.cluster_id).collect();
    assert_eq!(accepted, expected);
    assert!(batch.evictions.is_empty());
    controller
        .apply_drain_outcome(ShDrainOutcome {
            accepted,
            ..ShDrainOutcome::default()
        })
        .unwrap();
}

/// Reads 3 through the diamond 3 → {1, 2} → 0 and 4 directly through 0.
/// Both reads outlive departure and confirmed eviction of all three owners.
fn queued_behind_evicted_diamond() -> (ShResidencyController, ShClusterRequest, ShClusterRequest) {
    let mut c = controller(topology(
        (0..6).collect(),
        vec![vec![]; 6],
        vec![vec![], vec![0], vec![0], vec![1, 2], vec![0], vec![]],
        vec![1; 6],
    ));
    view(&mut c, &[3, 4], 0.0);
    assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 0);
    ready_requested(&mut c, 0);
    install_ready(&mut c, &[0]);
    c.promote_composed_clusters();
    for id in [1, 2] {
        assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, id);
        ready_requested(&mut c, id);
    }
    let direct = c.take_next_request().unwrap().unwrap();
    assert_eq!(direct.cluster_id, 4);
    install_ready(&mut c, &[1, 2]);
    c.promote_composed_clusters();
    let transitive = c.take_next_request().unwrap().unwrap();
    assert_eq!(transitive.cluster_id, 3);
    assert_ledger(&c);

    view(&mut c, &[], 0.1);
    view(&mut c, &[], 2.2);
    let batch = c.take_async_drain_batch().unwrap();
    assert!(batch.ready.is_empty());
    assert_eq!(
        batch.evictions.iter().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([0, 1, 2])
    );
    c.apply_drain_outcome(ShDrainOutcome {
        evicted: vec![0, 1, 2],
        ..ShDrainOutcome::default()
    })
    .unwrap();
    view(&mut c, &[3, 4], 2.3);
    assert_eq!(c.state(0), Some(ClusterResidencyState::Absent));
    assert!(c.states[0].failure.is_none());
    assert!(c.take_requests().unwrap().is_empty());
    assert_eq!(c.permits_in_use(), 2);
    (c, transitive, direct)
}

// Regression: a dependent's old read retained its permit and payload indefinitely
// after its evicted owner was reread and refused at a device limit.
#[test]
fn evicted_owner_waits_for_old_reads_then_refusal_releases_transitive_ready_work() {
    for completion in ["prepared", "failed", "cancelled"] {
        let (mut c, transitive, direct) = queued_behind_evicted_diamond();
        ready_requested(&mut c, direct.cluster_id);
        assert!(
            c.take_requests().unwrap().is_empty(),
            "transitive read still holds its owner"
        );
        let mut stale = transitive;
        stale.generation += 1;
        c.admit_cancelled_request(stale).unwrap();
        assert!(!c.admit_failed_request(stale).unwrap());
        let mut stale_payload = prepared(&c, transitive.cluster_id);
        stale_payload.generation = stale.generation;
        assert_eq!(
            c.admit_prepared(stale_payload).unwrap(),
            ShDrainAdmission::DroppedStale
        );
        assert!(c.take_requests().unwrap().is_empty());
        assert_eq!(c.permits_in_use(), 2);
        assert_ledger(&c);

        match completion {
            "prepared" => ready_requested(&mut c, transitive.cluster_id),
            "failed" => assert!(c.admit_failed_request(transitive).unwrap()),
            "cancelled" => c.admit_cancelled_request(transitive).unwrap(),
            _ => unreachable!(),
        }
        assert_ledger(&c);
        assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 0);
        ready_requested(&mut c, 0);
        refuse_batch(&mut c, &[0]);
        assert_eq!(c.state(0), Some(ClusterResidencyState::Failed));
        assert_eq!(c.state(4), Some(ClusterResidencyState::Absent));
        assert!(c.states[4].failure.is_none());
        if completion == "failed" {
            assert_eq!(c.state(3), Some(ClusterResidencyState::Failed));
            let failure = c.states[3].failure.unwrap();
            assert_eq!(failure.cause, FailureCause::Read);
            assert!(!failure.retry_spent);
        } else {
            assert_eq!(c.state(3), Some(ClusterResidencyState::Absent));
            assert!(c.states[3].failure.is_none());
        }
        assert_eq!(c.permits_in_use(), 0);
        assert_eq!(c.accounting().cpu.ready.current_bytes, 0);
        assert!(c.take_requests().unwrap().is_empty());
        assert_ledger(&c);
        let after_refusal = c.report_snapshot();
        c.admit_cancelled_request(transitive).unwrap();
        assert!(!c.admit_failed_request(transitive).unwrap());
        assert_eq!(c.report_snapshot(), after_refusal);

        leave_and_return(&mut c, 3, 3.0);
        assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 0);
        ready_requested(&mut c, 0);
        install_ready(&mut c, &[0]);
        assert!(c.take_requests().unwrap().is_empty());
        c.promote_composed_clusters();
        assert_eq!(
            c.take_requests()
                .unwrap()
                .iter()
                .map(|r| r.cluster_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        for id in [1, 2] {
            ready_requested(&mut c, id);
        }
        install_ready(&mut c, &[1, 2]);
        assert!(c.take_requests().unwrap().is_empty());
        c.promote_composed_clusters();
        let fresh = c.take_next_request().unwrap().unwrap();
        assert_eq!(fresh.cluster_id, 3);
        ready_requested(&mut c, 3);
        install_ready(&mut c, &[3]);
        c.promote_composed_clusters();
        assert!(c.all_targets_sampleable());
        assert_eq!(c.permits_in_use(), 0);
        assert_ledger(&c);
    }
}

#[test]
fn refusal_preflights_dependent_cleanup_before_evictions_and_other_outcome_changes() {
    for corrupt_permits in [false, true] {
        let (mut c, _, _) = queued_behind_evicted_diamond();
        for id in [3, 4] {
            ready_requested(&mut c, id);
        }
        assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 0);
        ready_requested(&mut c, 0);
        mark_sampleable(&mut c, 5);
        let batch = c.take_async_drain_batch().unwrap();
        assert_eq!(batch.evictions, vec![5]);
        assert_eq!(batch.ready.len(), 1);
        if corrupt_permits {
            c.permits_in_use = 1;
        } else {
            c.accounting.cpu.ready.current_bytes = 4;
        }
        let before = c.report_snapshot();
        let owners = c.request_owners.clone();
        let in_drain = c.in_drain.clone();
        let outcome = || ShDrainOutcome {
            refused: vec![0],
            evicted: vec![5],
            ..ShDrainOutcome::default()
        };
        assert!(matches!(
            c.apply_drain_outcome(outcome()),
            Err(ShResidencyControllerError::AccountingUnderflow(_))
        ));
        assert_eq!(c.report_snapshot(), before);
        assert_eq!(c.request_owners, owners);
        assert_eq!(c.in_drain, in_drain);
        assert_eq!(c.ready.keys().copied().collect::<Vec<_>>(), vec![3, 4]);
        assert_eq!(c.in_drain_evictions, BTreeSet::from([5]));
        assert!(c.states[0].failure.is_none());

        c.permits_in_use = 3;
        c.accounting.cpu.ready.current_bytes = 12;
        c.apply_drain_outcome(outcome()).unwrap();
        assert_eq!(c.state(5), Some(ClusterResidencyState::Absent));
        assert_eq!(c.permits_in_use(), 0);
        assert_eq!(c.accounting().cpu.ready.current_bytes, 0);
        assert_ledger(&c);
    }
}

#[test]
fn overlapping_refused_owners_release_their_shared_ready_dependent_once() {
    let (mut c, _, _) = queued_behind_evicted_diamond();
    for id in [3, 4] {
        ready_requested(&mut c, id);
    }
    assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 0);
    ready_requested(&mut c, 0);
    install_ready(&mut c, &[0]);
    c.promote_composed_clusters();
    for id in [1, 2] {
        assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, id);
        ready_requested(&mut c, id);
    }
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(
        batch
            .ready
            .iter()
            .map(|p| p.chunk.cluster_id)
            .collect::<Vec<_>>(),
        vec![1, 2, 4]
    );
    assert!(batch.evictions.is_empty());
    c.apply_drain_outcome(ShDrainOutcome {
        refused: vec![1, 2],
        accepted: vec![4],
        ..ShDrainOutcome::default()
    })
    .unwrap();
    for id in [1, 2] {
        assert_eq!(c.state(id), Some(ClusterResidencyState::Failed));
    }
    assert_eq!(c.state(3), Some(ClusterResidencyState::Absent));
    assert!(c.states[3].failure.is_none());
    assert_eq!(c.state(0), Some(ClusterResidencyState::Sampleable));
    assert_eq!(c.state(4), Some(ClusterResidencyState::InstalledUncomposed));
    assert_eq!(c.permits_in_use(), 0);
    assert_eq!(c.accounting().cpu.ready.current_bytes, 0);
    assert_ledger(&c);
}

// Regression: all permits could remain Ready behind an evicted owner, leaving
// no permit for the reread that would make any of those payloads installable.
#[test]
fn full_ready_permits_reclaim_only_dependents_for_mandatory_and_optional_owner_reads() {
    for optional in [false, true] {
        let mut owners = vec![vec![], vec![0], vec![0]];
        owners.extend(vec![vec![1, 2]; 7]);
        owners.push(vec![]);
        let mut c = controller(topology(
            (0..11).collect(),
            vec![vec![]; 11],
            owners,
            vec![1; 11],
        ));
        let visible: Vec<_> = (3..11).collect();
        view(&mut c, &visible, 0.0);
        assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 0);
        ready_requested(&mut c, 0);
        install_ready(&mut c, &[0]);
        c.promote_composed_clusters();
        for id in [1, 2] {
            assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, id);
            ready_requested(&mut c, id);
        }
        install_ready(&mut c, &[1, 2]);
        c.promote_composed_clusters();
        assert_eq!(c.take_requests().unwrap().len(), MAX_STREAM_PERMITS);
        view(&mut c, &[10], 0.1);
        view(&mut c, &[10], 2.2);
        let batch = c.take_async_drain_batch().unwrap();
        assert_eq!(
            batch.evictions.iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([0, 1, 2])
        );
        c.apply_drain_outcome(ShDrainOutcome {
            evicted: vec![0, 1, 2],
            ..ShDrainOutcome::default()
        })
        .unwrap();
        if optional {
            let rows: Vec<_> = visible.iter().map(|&id| (0, id, 24)).collect();
            let set = residency_set(11, &rows, 32);
            let stage = CellDemand::new(set.max_lead);
            let none_visible = VisibleCells::Culled(vec![]);
            c.update_targets(
                &none_visible,
                Some(stage.frame(&set, 0, PORTAL, &none_visible)),
                2.3,
            )
            .unwrap();
        } else {
            view(&mut c, &visible, 2.3);
        }
        assert!(c.take_requests().unwrap().is_empty());
        for id in &visible {
            ready_requested(&mut c, *id);
        }
        assert_eq!(c.permits_in_use(), MAX_STREAM_PERMITS);
        assert_ledger(&c);

        c.counters.rereads = u64::MAX;
        let before = c.report_snapshot();
        let held_owners = c.request_owners.clone();
        assert!(matches!(
            c.take_next_request(),
            Err(ShResidencyControllerError::AccountingOverflow(
                "stream re-reads"
            ))
        ));
        assert_eq!(c.report_snapshot(), before);
        assert_eq!(c.request_owners, held_owners);
        assert_eq!(c.ready.len(), MAX_STREAM_PERMITS);
        c.counters.rereads = 0;

        let owner = c.take_next_request().unwrap().unwrap();
        assert_eq!(owner.cluster_id, 0);
        assert_eq!(owner.mandatory, !optional);
        assert_eq!(
            c.permits_in_use(),
            if optional {
                MAX_OPTIONAL_STREAM_PERMITS
            } else {
                MAX_STREAM_PERMITS
            }
        );
        assert_eq!(c.state(10), Some(ClusterResidencyState::Ready));
        assert_eq!(c.state(3), Some(ClusterResidencyState::Absent));
        assert!(c.states[3].failure.is_none());
        assert_ledger(&c);
        ready_requested(&mut c, 0);
        let batch = c.take_async_drain_batch().unwrap();
        assert_eq!(
            batch
                .ready
                .iter()
                .map(|p| p.chunk.cluster_id)
                .collect::<Vec<_>>(),
            vec![0, 10]
        );
        let unrelated = batch
            .ready
            .into_iter()
            .find(|p| p.chunk.cluster_id == 10)
            .unwrap();
        c.apply_drain_outcome(ShDrainOutcome {
            refused: vec![0],
            deferred: vec![unrelated],
            ..ShDrainOutcome::default()
        })
        .unwrap();
        assert_eq!(c.permits_in_use(), 1);
        assert_eq!(c.accounting().cpu.ready.current_bytes, 4);
        assert_eq!(c.state(10), Some(ClusterResidencyState::Ready));
        for id in 3..10 {
            assert_eq!(c.state(id), Some(ClusterResidencyState::Absent));
            assert!(c.states[id as usize].failure.is_none());
        }
        assert_ledger(&c);
        assert!(c.take_requests().unwrap().is_empty());
        install_ready(&mut c, &[10]);
        assert_eq!(c.permits_in_use(), 0);
        assert_ledger(&c);
    }
}

// Regression: after mandatory reads became optional, neither of two owner
// cohorts could alone free the five permits needed to restore the reservation.
#[test]
fn optional_owner_recovery_reclaims_across_stalled_cohorts_and_preserves_other_work() {
    for unrelated in [false, true] {
        let cohort_size = if unrelated { 3 } else { 4 };
        let mut owners = vec![vec![], vec![]];
        for owner in [0, 1] {
            owners.extend(vec![vec![owner]; cohort_size]);
        }
        if unrelated {
            owners.extend(vec![vec![], vec![]]);
        }
        let mut c = controller(topology(
            (0..10).collect(),
            vec![vec![]; 10],
            owners,
            vec![1; 10],
        ));
        let leaves: Vec<_> = (2..10).collect();
        view(&mut c, &leaves, 0.0);
        for owner in [0, 1] {
            assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, owner);
            ready_requested(&mut c, owner);
        }
        install_ready(&mut c, &[0, 1]);
        c.promote_composed_clusters();
        let old_requests = c.take_requests().unwrap();
        assert_eq!(old_requests.len(), MAX_STREAM_PERMITS);
        assert!(old_requests.iter().all(|r| r.mandatory));
        let remaining_view: &[u32] = if unrelated { &[8, 9] } else { &[] };
        view(&mut c, remaining_view, 0.1);
        view(&mut c, remaining_view, 2.2);
        let batch = c.take_async_drain_batch().unwrap();
        assert_eq!(batch.evictions, vec![0, 1]);
        c.apply_drain_outcome(ShDrainOutcome {
            evicted: batch.evictions,
            ..ShDrainOutcome::default()
        })
        .unwrap();
        let rows: Vec<_> = leaves.iter().map(|&id| (0, id, 24)).collect();
        let set = residency_set(10, &rows, 32);
        let stage = CellDemand::new(set.max_lead);
        let none_visible = VisibleCells::Culled(vec![]);
        c.update_targets(
            &none_visible,
            Some(stage.frame(&set, 0, PORTAL, &none_visible)),
            2.3,
        )
        .unwrap();
        for owner in [0, 1] {
            assert_eq!(c.states[owner].class, Some(TargetClass::Band));
        }
        for &id in &leaves {
            if !unrelated || id != 9 {
                ready_requested(&mut c, id);
            }
        }
        assert_eq!(c.permits_in_use(), MAX_STREAM_PERMITS);
        assert_ledger(&c);

        for owner in [0, 1] {
            let request = c
                .take_next_request()
                .unwrap()
                .expect("blocked cohorts must free an owner read");
            assert_eq!(request.cluster_id, owner);
            assert!(!request.mandatory);
            assert_eq!(c.permits_in_use(), MAX_OPTIONAL_STREAM_PERMITS);
            if unrelated {
                assert_eq!(c.state(8), Some(ClusterResidencyState::Ready));
                let old = *old_requests.iter().find(|r| r.cluster_id == 9).unwrap();
                assert!(c.matches_queued_request(old));
            }
            assert_ledger(&c);
        }
        for owner in [0, 1] {
            ready_requested(&mut c, owner);
        }
        let batch = c.take_async_drain_batch().unwrap();
        assert_eq!(
            batch
                .ready
                .iter()
                .map(|p| p.chunk.cluster_id)
                .collect::<Vec<_>>(),
            if unrelated { vec![0, 1, 8] } else { vec![0, 1] }
        );
        c.apply_drain_outcome(ShDrainOutcome {
            accepted: vec![0, 1],
            deferred: batch
                .ready
                .into_iter()
                .filter(|p| p.chunk.cluster_id == 8)
                .collect(),
            ..ShDrainOutcome::default()
        })
        .unwrap();
        c.promote_composed_clusters();
        if unrelated {
            ready_requested(&mut c, 9);
        }
        install_ready(&mut c, &[8, 9]);
        c.promote_composed_clusters();
        assert_eq!(c.permits_in_use(), 0);
        assert_ledger(&c);

        // Every reclaimed payload can be read afresh once its owner is sampleable.
        for _ in 0..2 {
            let requests = c.take_requests().unwrap();
            assert!(!requests.is_empty());
            assert!(requests.len() <= MAX_OPTIONAL_STREAM_PERMITS);
            let ids: Vec<_> = requests.iter().map(|r| r.cluster_id).collect();
            for &id in &ids {
                assert!(c.states[id as usize].failure.is_none());
                ready_requested(&mut c, id);
            }
            install_ready(&mut c, &ids);
            c.promote_composed_clusters();
            assert_ledger(&c);
        }
        assert!(c.all_targets_sampleable());
        assert_eq!(c.permits_in_use(), 0);
    }
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
