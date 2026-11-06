//! Device-limit outcomes at the renderer/controller seam.

use super::tests::{controller, hinted_topology, mark_sampleable, prepared, topology};
use super::*;
use crate::lightmap_streaming::test_fixtures::{PORTAL, residency_set};
use crate::sh_streaming::generation::FixedGenerationClock;
use crate::sh_streaming::sync_manifest_test_fixture;
use crate::sh_streaming::topology::SeamPortalEndpoint;
use crate::streaming::cell_demand::CellDemand;
use postretro_visibility::VisibleCells;

fn independent(count: usize) -> ShResidencyController {
    controller(topology((0..count as u32).collect(), vec![vec![]; count]))
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
    assert_eq!(controller.cpu().ready.current_bytes, bytes);
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
            + b.at_whole_map_cap
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
    assert_eq!(c.cpu().ready.current_bytes, 4);
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
    c.cpu.ready.current_bytes = 0;
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

fn whole_map_refusal(controller: &mut ShResidencyController, cluster_id: u32) {
    let request = controller.take_next_request().unwrap().unwrap();
    assert_eq!(request.cluster_id, cluster_id);
    ready_requested(controller, cluster_id);
    assert_eq!(controller.take_async_drain_batch().unwrap().ready.len(), 1);
    controller
        .apply_drain_outcome(ShDrainOutcome {
            whole_map_refused: vec![cluster_id],
            ..ShDrainOutcome::default()
        })
        .unwrap();
}

// The cap is no device limit, so its misses must not read as one.
#[test]
fn whole_map_refusal_counts_its_own_bucket() {
    let mut c = independent(1);
    view(&mut c, &[0], 0.0);
    whole_map_refusal(&mut c, 0);
    assert_eq!(c.state(0), Some(ClusterResidencyState::Failed));
    assert_ledger(&c);
    view(&mut c, &[], 0.2);
    view(&mut c, &[0], 0.3);
    assert_eq!(c.counters().miss_buckets.at_whole_map_cap, 1);
    assert_eq!(c.counters().miss_buckets.device_limited, 0);
    assert_bucket_total(&c);
}

// Holes open as residents leave, so it retries as a device refusal does.
#[test]
fn whole_map_refusal_retries_after_leaving_and_reentering_targets() {
    let mut c = independent(1);
    view(&mut c, &[0], 0.0);
    whole_map_refusal(&mut c, 0);
    view(&mut c, &[0], 5.0);
    assert_eq!(c.state(0), Some(ClusterResidencyState::Failed));
    leave_and_return(&mut c, 0, 6.0);
    assert_eq!(c.state(0), Some(ClusterResidencyState::Absent));
}

#[test]
fn settling_refusal_holds_mandatory_and_reveal_counts_first_device_limit_miss() {
    let mut c = independent(1);
    c.set_full_demand(true);
    view(&mut c, &[0], 0.0);
    refusal(&mut c, 0);
    view(&mut c, &[0], 10.0);
    assert_eq!(c.unsettled_targets(), Some(1));
    assert_eq!(c.counters().misses, 0);
    c.set_full_demand(false);
    view(&mut c, &[0], 10.1);
    assert_eq!(c.counters().miss_buckets.device_limited, 1);
    assert_eq!(c.unsettled_targets(), Some(1));
    assert_bucket_total(&c);
}

#[test]
fn refusal_in_reveal_drain_preserves_the_reveal_frames_existing_miss() {
    let mut c = independent(1);
    c.set_full_demand(true);
    view(&mut c, &[0], 0.0);
    c.take_next_request().unwrap().unwrap();
    ready_requested(&mut c, 0);
    c.set_full_demand(false);
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
        vec![SeamPortalEndpoint {
            portal_id: 0,
            front_cluster_id: 0,
            back_cluster_id: 1,
        }],
        BTreeSet::new(),
        vec![],
    ));
    c.set_full_demand(true);
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
fn refusal_blocks_only_the_refused_cluster_until_it_leaves_and_returns() {
    let mut c = independent(2);
    view(&mut c, &[0, 1], 0.0);
    refusal(&mut c, 0);
    for now in [0.1, 1.0, 3.0] {
        view(&mut c, &[0, 1], now);
        assert_eq!(c.state(0), Some(ClusterResidencyState::Failed));
        assert_eq!(c.cpu().ready.current_bytes, 0);
    }
    assert_eq!(
        c.take_next_request().unwrap().unwrap().cluster_id,
        1,
        "a refusal holds no other cluster back"
    );
    assert!(c.take_requests().unwrap().is_empty());
    ready_requested(&mut c, 1);
    install_ready(&mut c, &[1]);
    c.promote_composed_clusters();
    assert_eq!(c.state(1), Some(ClusterResidencyState::Sampleable));

    view(&mut c, &[], 3.1);
    view(&mut c, &[], 5.11);
    view(&mut c, &[0, 1], 5.2);
    let requests = c.take_requests().unwrap();
    assert_eq!(requests.first().unwrap().cluster_id, 0);
    for request in &requests {
        ready_requested(&mut c, request.cluster_id);
    }
    install_ready(
        &mut c,
        &requests.iter().map(|r| r.cluster_id).collect::<Vec<_>>(),
    );
    c.promote_composed_clusters();
    assert!(c.all_targets_sampleable());
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

/// Clusters 0, 1 and 2 resident, departed and evicted while reads of 3 and 4
/// stay in flight. Cluster 0 is then back in view, read afresh beside them.
fn old_reads_outlive_eviction() -> (ShResidencyController, ShClusterRequest, ShClusterRequest) {
    let mut c = independent(6);
    view(&mut c, &[0, 1, 2], 0.0);
    assert_eq!(c.take_requests().unwrap().len(), 3);
    for id in [0, 1, 2] {
        ready_requested(&mut c, id);
    }
    install_ready(&mut c, &[0, 1, 2]);
    c.promote_composed_clusters();
    view(&mut c, &[0, 1, 2, 3, 4], 0.05);
    let old_three = c.take_next_request().unwrap().unwrap();
    assert_eq!(old_three.cluster_id, 3);
    let old_four = c.take_next_request().unwrap().unwrap();
    assert_eq!(old_four.cluster_id, 4);
    assert_ledger(&c);

    view(&mut c, &[3, 4], 0.1);
    view(&mut c, &[3, 4], 2.2);
    let batch = c.take_async_drain_batch().unwrap();
    assert!(batch.ready.is_empty());
    assert_eq!(batch.evictions, vec![0, 1, 2]);
    c.apply_drain_outcome(ShDrainOutcome {
        evicted: vec![0, 1, 2],
        ..ShDrainOutcome::default()
    })
    .unwrap();
    view(&mut c, &[0, 3, 4], 2.3);
    assert_eq!(c.state(0), Some(ClusterResidencyState::Absent));
    assert!(c.states[0].failure.is_none());
    assert_eq!(c.permits_in_use(), 2);
    (c, old_three, old_four)
}

// Regression: an old read retained its permit and payload indefinitely after
// the cluster it waited on was reread and refused at a device limit.
#[test]
fn a_reread_refused_beside_old_reads_leaves_each_read_to_finish_on_its_own() {
    for completion in ["prepared", "failed", "cancelled"] {
        let (mut c, old_three, old_four) = old_reads_outlive_eviction();
        ready_requested(&mut c, old_four.cluster_id);
        let mut stale = old_three;
        stale.generation += 1;
        c.admit_cancelled_request(stale).unwrap();
        assert!(!c.admit_failed_request(stale).unwrap());
        let mut stale_payload = prepared(&c, old_three.cluster_id);
        stale_payload.generation = stale.generation;
        assert_eq!(
            c.admit_prepared(stale_payload).unwrap(),
            ShDrainAdmission::DroppedStale
        );
        assert_eq!(c.permits_in_use(), 2);
        assert_ledger(&c);

        let reread = c.take_next_request().unwrap().unwrap();
        assert_eq!(
            reread.cluster_id, 0,
            "no read waits on another cluster's read"
        );
        assert_eq!(c.counters().rereads, 1);
        ready_requested(&mut c, 0);
        let batch = c.take_async_drain_batch().unwrap();
        assert_eq!(batch.ready.len(), 2);
        c.apply_drain_outcome(ShDrainOutcome {
            refused: vec![0],
            accepted: vec![4],
            ..ShDrainOutcome::default()
        })
        .unwrap();
        assert_eq!(c.state(0), Some(ClusterResidencyState::Failed));
        assert_eq!(c.state(4), Some(ClusterResidencyState::InstalledUncomposed));
        assert!(c.states[4].failure.is_none());
        assert_eq!(c.state(3), Some(ClusterResidencyState::Queued));
        assert!(c.matches_queued_request(old_three));

        match completion {
            "prepared" => ready_requested(&mut c, old_three.cluster_id),
            "failed" => assert!(c.admit_failed_request(old_three).unwrap()),
            "cancelled" => c.admit_cancelled_request(old_three).unwrap(),
            _ => unreachable!(),
        }
        assert_ledger(&c);
        match completion {
            "prepared" => assert_eq!(c.state(3), Some(ClusterResidencyState::Ready)),
            "failed" => {
                assert_eq!(c.state(3), Some(ClusterResidencyState::Failed));
                let failure = c.states[3].failure.unwrap();
                assert_eq!(failure.cause, FailureCause::Read);
                assert!(!failure.retry_spent);
            }
            _ => {
                assert_eq!(c.state(3), Some(ClusterResidencyState::Absent));
                assert!(c.states[3].failure.is_none());
            }
        }
        let after_completion = c.report_snapshot();
        c.admit_cancelled_request(old_three).unwrap();
        assert!(!c.admit_failed_request(old_three).unwrap());
        assert_eq!(c.report_snapshot(), after_completion);

        leave_and_return(&mut c, 0, 3.0);
        assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 0);
        ready_requested(&mut c, 0);
        install_ready(&mut c, &[0]);
        c.promote_composed_clusters();
        assert!(c.all_targets_sampleable());
        assert_eq!(c.permits_in_use(), 0);
        assert_ledger(&c);
    }
}

#[test]
fn refusal_preflight_rejects_the_whole_outcome_before_evictions_and_other_changes() {
    for corrupt_permits in [false, true] {
        let (mut c, _, _) = old_reads_outlive_eviction();
        assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 0);
        ready_requested(&mut c, 0);
        mark_sampleable(&mut c, 5);
        let batch = c.take_async_drain_batch().unwrap();
        assert_eq!(batch.evictions, vec![5]);
        assert_eq!(batch.ready.len(), 1);
        if corrupt_permits {
            c.permits_in_use = 0;
        } else {
            c.cpu.ready.current_bytes = 0;
        }
        let before = c.report_snapshot();
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
        assert_eq!(c.in_drain, in_drain);
        assert_eq!(c.in_drain_evictions, BTreeSet::from([5]));
        assert!(c.states[0].failure.is_none());

        c.permits_in_use = 3;
        c.cpu.ready.current_bytes = 4;
        c.apply_drain_outcome(outcome()).unwrap();
        assert_eq!(c.state(5), Some(ClusterResidencyState::Absent));
        assert_eq!(c.state(0), Some(ClusterResidencyState::Failed));
        assert_eq!(c.permits_in_use(), 2, "the old reads keep their permits");
        assert_eq!(c.cpu().ready.current_bytes, 0);
        assert_ledger(&c);
    }
}

#[test]
fn two_refusals_in_one_outcome_leave_accepted_and_resident_work_alone() {
    let mut c = independent(4);
    view(&mut c, &[0, 1, 2, 3], 0.0);
    mark_sampleable(&mut c, 0);
    assert_eq!(c.take_requests().unwrap().len(), 3);
    for id in [1, 2, 3] {
        ready_requested(&mut c, id);
    }
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.ready.len(), 3);
    assert!(batch.evictions.is_empty());
    c.apply_drain_outcome(ShDrainOutcome {
        refused: vec![1, 2],
        accepted: vec![3],
        ..ShDrainOutcome::default()
    })
    .unwrap();
    for id in [1, 2] {
        assert_eq!(c.state(id), Some(ClusterResidencyState::Failed));
    }
    assert_eq!(c.state(0), Some(ClusterResidencyState::Sampleable));
    assert_eq!(c.state(3), Some(ClusterResidencyState::InstalledUncomposed));
    assert_eq!(c.permits_in_use(), 0);
    assert_eq!(c.cpu().ready.current_bytes, 0);
    assert_ledger(&c);
}

// Regression: all permits could sit Ready, leaving no permit for the next read.
// Every Ready payload installs on its own, so a drain frees them.
#[test]
fn full_ready_permits_drain_then_free_mandatory_and_optional_reads() {
    for optional in [false, true] {
        let mut c = independent(11);
        let visible: Vec<_> = (3..11).collect();
        view(&mut c, &visible, 0.0);
        assert_eq!(c.take_requests().unwrap().len(), MAX_STREAM_PERMITS);
        for &id in &visible {
            ready_requested(&mut c, id);
        }
        // Cluster 0 is next, at its class, and was evicted once.
        c.states[0].evicted = true;
        if optional {
            let rows: Vec<_> = std::iter::once(0)
                .chain(visible.iter().copied())
                .map(|id| (0, id, 24))
                .collect();
            let set = residency_set(11, &rows, 32);
            let stage = CellDemand::new(set.max_lead);
            let all = VisibleCells::Culled(visible.clone());
            c.update_targets(&all, Some(stage.frame(&set, 0, PORTAL, &all)), 0.1)
                .unwrap();
        } else {
            let mut wider = visible.clone();
            wider.push(0);
            view(&mut c, &wider, 0.1);
        }
        assert_eq!(c.permits_in_use(), MAX_STREAM_PERMITS);
        assert!(
            c.take_next_request().unwrap().is_none(),
            "every permit is held"
        );
        assert_ledger(&c);

        // Five payloads stay: past the optional reservation, inside the limit.
        let batch = c.take_async_drain_batch().unwrap();
        let mut ready = batch.ready.into_iter();
        let accepted: Vec<_> = ready.by_ref().take(3).map(|p| p.chunk.cluster_id).collect();
        c.apply_drain_outcome(ShDrainOutcome {
            accepted,
            deferred: ready.collect(),
            ..ShDrainOutcome::default()
        })
        .unwrap();
        assert_eq!(c.permits_in_use(), 5);
        assert_ledger(&c);

        if optional {
            assert!(
                c.take_next_request().unwrap().is_none(),
                "optional reads stop at the reservation"
            );
            let batch = c.take_async_drain_batch().unwrap();
            let accepted = batch.ready.iter().map(|p| p.chunk.cluster_id).collect();
            c.apply_drain_outcome(ShDrainOutcome {
                accepted,
                ..ShDrainOutcome::default()
            })
            .unwrap();
        }

        c.counters.rereads = u64::MAX;
        let before = c.report_snapshot();
        assert!(matches!(
            c.take_next_request(),
            Err(ShResidencyControllerError::AccountingOverflow(
                "stream re-reads"
            ))
        ));
        assert_eq!(c.report_snapshot(), before);
        assert_eq!(c.state(0), Some(ClusterResidencyState::Absent));
        c.counters.rereads = 0;

        let request = c.take_next_request().unwrap().unwrap();
        assert_eq!(request.cluster_id, 0);
        assert_eq!(request.mandatory, !optional);
        assert_eq!(c.counters().rereads, 1);
        assert_ledger(&c);
    }
}

#[test]
fn device_limit_and_read_failure_each_count_for_their_own_cluster() {
    for read_failure_first in [false, true] {
        let mut c = independent(2);
        let (failed, refused) = if read_failure_first { (0, 1) } else { (1, 0) };
        // Prepare both failure causes while not presenting a frame.
        c.set_full_demand(true);
        view(&mut c, &[0, 1], 0.0);
        let requests = c.take_requests().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            c.admit_failed_request(*requests.iter().find(|r| r.cluster_id == failed).unwrap())
                .unwrap()
        );
        ready_requested(&mut c, refused);
        refuse_batch(&mut c, &[refused]);
        c.set_full_demand(false);
        view(&mut c, &[0, 1], 0.1);
        assert_eq!(c.counters().miss_buckets.device_limited, 1);
        assert_eq!(c.counters().miss_buckets.failed, 1);
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
fn refusal_evicts_no_pin_lead_visible_or_optional_resident_while_nothing_mandatory_is_ready() {
    let mut c = controller(hinted_topology(
        vec![0, 1, 2, 3, 4, 5],
        vec![vec![]; 6],
        vec![],
        BTreeSet::from([1]),
        vec![],
    ));
    let set = residency_set(6, &[(0, 2, 4), (0, 4, 24)], 32);
    let stage = CellDemand::new(set.max_lead);
    let visible = VisibleCells::Culled(vec![0, 3, 5]);
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
        assert!(batch.evictions.is_empty() && batch.victims.is_empty());
        assert!(batch.ready.is_empty());
        c.apply_drain_outcome(ShDrainOutcome::default()).unwrap();
        for id in [1, 2, 3, 4] {
            assert_eq!(c.state(id), Some(ClusterResidencyState::Sampleable));
        }
    }
}

#[test]
fn a_device_refused_optional_is_no_victim_and_keeps_its_target() {
    let mut c = controller(hinted_topology(
        vec![0, 1, 2],
        vec![vec![]; 3],
        vec![SeamPortalEndpoint {
            portal_id: 0,
            front_cluster_id: 0,
            back_cluster_id: 1,
        }],
        BTreeSet::new(),
        vec![],
    ));
    view(&mut c, &[0], 0.0);
    // Read optional 1 while 0 is in flight, so its install attempt happens.
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

    // Mandatory work reaches a full pool: a cluster holding nothing can free
    // nothing, and it must not lose the target its retry waits on.
    view(&mut c, &[0, 2], 0.1);
    assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 2);
    ready_requested(&mut c, 2);
    let batch = c.take_async_drain_batch().unwrap();
    assert!(batch.evictions.is_empty() && batch.victims.is_empty());
    assert!(c.is_targeted(1));
    assert!(!c.states[1].suppressed);
}

#[test]
fn early_seam_reentry_never_wakes_on_time_alone_and_later_reentry_does() {
    let mut c = controller(hinted_topology(
        vec![0, 1, 2],
        vec![vec![]; 3],
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
fn rapid_horizon_churn_keeps_targeted_refusal_failed() {
    let mut c = controller(topology(vec![0, 1, 2], vec![vec![]; 3]));
    view(&mut c, &[1, 2], 0.0);
    for id in [1, 2] {
        mark_sampleable(&mut c, id);
    }
    view(&mut c, &[0, 1], 0.0);
    refusal(&mut c, 0);
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
fn pool_refusals_of_band_work_cannot_churn_a_refused_band_cluster() {
    let mut c = controller(topology(vec![0, 1, 2, 3], vec![vec![]; 4]));
    view(&mut c, &[2, 3], 0.0);
    for id in [2, 3] {
        mark_sampleable(&mut c, id);
    }
    let set = residency_set(4, &[(2, 0, 24), (2, 1, 24), (3, 0, 24), (3, 1, 24)], 32);
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
    let mut pool_refusals = 0;
    for tick in 1..=400 {
        let now = f64::from(tick) / 60.0;
        // Each pair has a stable horizon frame in which the band stays out,
        // followed by a changed horizon that restores it to be refused again.
        let camera = if (tick / 2) % 2 == 0 { 2 } else { 3 };
        update(&mut c, camera, now);
        let batch = c.take_async_drain_batch().unwrap();
        assert!(batch.evictions.is_empty());
        c.apply_drain_outcome(ShDrainOutcome::default()).unwrap();
        for request in c.take_requests().unwrap() {
            assert_eq!(request.cluster_id, 1);
            ready_requested(&mut c, 1);
            let batch = c.take_async_drain_batch().unwrap();
            assert_eq!(batch.ready.len(), 1);
            c.apply_drain_outcome(ShDrainOutcome {
                pool_refused: vec![1],
                ..ShDrainOutcome::default()
            })
            .unwrap();
            pool_refusals += 1;
        }
        assert_eq!(c.state(1), Some(ClusterResidencyState::Absent));
        assert_eq!(c.permits_in_use(), 0);
        assert_eq!(c.cpu().ready.current_bytes, 0);
        // Yielding skips the refused cluster, so it stays targeted and never
        // earns the leave-and-return retry.
        assert_eq!(c.state(0), Some(ClusterResidencyState::Failed));
        assert!(c.is_targeted(0));
        assert!(!c.states[0].suppressed, "only a refused install yields");
    }
    assert!(pool_refusals > 0);
    assert_eq!(c.counters().retries, 0);
}

#[test]
fn newly_visible_refused_cluster_counts_device_limited_before_it_reenters_the_target_set() {
    let mut c = independent(2);
    c.set_full_demand(true);
    view(&mut c, &[1], 0.0);
    refusal(&mut c, 1);
    view(&mut c, &[], 0.1);
    view(&mut c, &[], 2.2);
    assert!(!c.is_targeted(1));
    c.set_full_demand(false);
    view(&mut c, &[1], 2.3);
    assert_eq!(c.counters().miss_buckets.device_limited, 1);
    assert_eq!(c.counters().miss_buckets.outside_reach, 0);
    assert_bucket_total(&c);
}

#[test]
fn a_victim_evicted_in_a_refusal_drain_triggers_no_further_eviction() {
    let mut c = controller(topology(vec![0, 1, 2], vec![vec![]; 3]));
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
        batch.victims,
        vec![1],
        "the renderer evicts before it refuses"
    );
    assert_eq!(batch.ready.len(), 1);
    c.apply_drain_outcome(ShDrainOutcome {
        evicted: vec![1],
        refused: vec![0],
        ..ShDrainOutcome::default()
    })
    .unwrap();
    let later = c.take_async_drain_batch().unwrap();
    assert!(later.evictions.is_empty() && later.victims.is_empty());
    assert!(later.ready.is_empty());
    assert_eq!(c.state(2), Some(ClusterResidencyState::Sampleable));
    assert_ledger(&c);
}

#[test]
fn a_horizon_change_lifting_suppression_does_not_retry_a_targeted_refusal() {
    let mut c = controller(topology(vec![0, 1, 2, 3], vec![vec![]; 4]));
    mark_sampleable(&mut c, 2);
    let set = residency_set(4, &[(2, 0, 24), (2, 1, 24)], 32);
    let stage = CellDemand::new(set.max_lead);
    let update = |c: &mut ShResidencyController, drawn: &[u32], now| {
        let visible = VisibleCells::Culled(drawn.to_vec());
        c.update_targets(&visible, Some(stage.frame(&set, 2, PORTAL, &visible)), now)
            .unwrap();
    };
    update(&mut c, &[2], 0.0);
    refusal(&mut c, 0);
    assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 1);
    ready_requested(&mut c, 1);
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.ready.len(), 1);
    c.apply_drain_outcome(ShDrainOutcome {
        pool_refused: vec![1],
        ..ShDrainOutcome::default()
    })
    .unwrap();
    assert!(c.states[1].suppressed);
    update(&mut c, &[2], 0.1);
    assert!(!c.is_targeted(1));
    mark_sampleable(&mut c, 3);
    update(&mut c, &[2, 3], 3.0);
    assert!(c.is_targeted(0));
    assert!(c.is_targeted(1));
    assert_eq!(c.state(0), Some(ClusterResidencyState::Failed));
    assert_eq!(
        c.take_requests()
            .unwrap()
            .iter()
            .map(|r| r.cluster_id)
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(c.counters().retries, 0);
}
