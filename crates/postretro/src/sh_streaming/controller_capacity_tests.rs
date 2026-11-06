//! The controller's half of capacity admission: who goes first, and what
//! becomes of a cluster that yields. What a free is worth is the renderer's.
//! See: context/lib/rendering_pipeline.md §4 (Bounded SH pool)

use std::collections::BTreeSet;

use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_visibility::{VisibilityPath, VisibleCells};

use super::super::topology::SeamPortalEndpoint;
use super::tests::{
    accept_evicting, controller, hinted_topology, mark_sampleable, prepared, queue_ready, topology,
};
use super::*;
use crate::lightmap_streaming::test_fixtures::{PORTAL, residency_set};
use crate::sh_streaming::generation::FixedGenerationClock;
use crate::streaming::cell_demand::{CellDemand, DemandFrame};

const FRUSTUM: VisibilityPath = VisibilityPath::SolidCellFallback;

/// Camera cell 0: lead 1, band 2 and 3 (3 the farther), one cluster per cell.
fn corridor() -> CellResidencySetSection {
    residency_set(8, &[(0, 0, 0), (0, 1, 4), (0, 2, 20), (0, 3, 28)], 32)
}

fn level() -> ShResidencyController {
    controller(topology((0..8).collect(), vec![Vec::new(); 8]))
}

fn frame(
    c: &mut ShResidencyController,
    set: &CellResidencySetSection,
    camera: u32,
    path: VisibilityPath,
    drawn: &[u32],
    now: f64,
) {
    let cells = VisibleCells::Culled(drawn.to_vec());
    let stage = CellDemand::new(set.max_lead);
    c.update_targets(&cells, Some(stage.frame(set, camera, path, &cells)), now)
        .unwrap();
}

fn admit(c: &mut ShResidencyController, cluster_id: u32) {
    assert_eq!(
        c.admit_prepared(prepared(c, cluster_id)).unwrap(),
        ShDrainAdmission::Ready
    );
}

fn refuse_pool(c: &mut ShResidencyController, cluster_id: u32) {
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.ready.len(), 1);
    c.apply_drain_outcome(ShDrainOutcome {
        pool_refused: vec![cluster_id],
        ..ShDrainOutcome::default()
    })
    .unwrap();
}

#[test]
fn victims_run_band_and_seam_by_the_pressure_comparator_then_hysteresis() {
    let mut c = controller(hinted_topology(
        (0..8).collect(),
        vec![Vec::new(); 8],
        vec![SeamPortalEndpoint {
            portal_id: 0,
            front_cluster_id: 0,
            back_cluster_id: 4,
        }],
        BTreeSet::new(),
        Vec::new(),
    ));
    let set = corridor();
    frame(&mut c, &set, 0, PORTAL, &[0, 5], 0.0);
    for id in 1..6 {
        mark_sampleable(&mut c, id);
    }
    // Cluster 5 leaves the view and retains only hysteresis.
    frame(&mut c, &set, 0, PORTAL, &[0], 0.1);
    assert_eq!(c.states[5].class, Some(TargetClass::Hysteresis));
    assert_eq!(c.states[4].class, Some(TargetClass::SeamWarm));
    assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 0);
    admit(&mut c, 0);

    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.victims, vec![3, 2, 4, 5]);
    assert_eq!(batch.mandatory, vec![0, 1]);
    assert_eq!(c.in_drain_victims.len(), 4);
}

#[test]
fn a_drain_with_no_mandatory_work_ready_names_no_capacity_policy() {
    let mut c = level();
    let set = corridor();
    frame(&mut c, &set, 0, PORTAL, &[0], 0.0);
    for id in 0..4 {
        mark_sampleable(&mut c, id);
    }
    // A full pool, and optional work the only thing ready.
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(
        (batch.mandatory.capacity(), batch.victims.capacity()),
        (0, 0),
        "an idle drain allocates nothing for capacity"
    );

    c.states[2].state = ClusterResidencyState::Queued;
    c.permits_in_use = 1;
    admit(&mut c, 2);
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.ready.len(), 1);
    assert!(batch.mandatory.is_empty() && batch.victims.is_empty());
}

#[test]
fn sync_proof_names_mandatory_work_but_no_victim() {
    let mut c = level();
    let set = corridor();
    frame(&mut c, &set, 0, PORTAL, &[0], 0.0);
    queue_ready(&mut c, 0);
    for id in 1..4 {
        mark_sampleable(&mut c, id);
    }
    let batch = c.take_drain_batch().unwrap();
    assert_eq!(batch.mandatory, vec![0, 1]);
    assert!(
        batch.victims.is_empty(),
        "the capture baseline evicts nothing"
    );
}

// Regression: held drawn residents were Visible, so one clip made them
// mandatory and growth sized to them.
#[test]
fn held_drawn_residents_stay_optional_and_yield_last_to_lead() {
    let mut c = level();
    let set = corridor();
    frame(&mut c, &set, 0, PORTAL, &[0, 4], 0.0);
    for id in [0, 1, 2, 3, 4, 5] {
        mark_sampleable(&mut c, id);
    }
    frame(&mut c, &set, 0, PORTAL, &[0], 0.1);
    // A held frame draws band 2 and stray resident 5. Lead 1 reads again.
    frame(&mut c, &set, 0, FRUSTUM, &[2, 5], 0.2);
    assert_eq!(c.states[2].class, Some(TargetClass::Band));
    assert_eq!(c.states[4].class, Some(TargetClass::Hysteresis));
    assert_eq!(c.states[5].class, Some(TargetClass::Held));
    c.states[1].state = ClusterResidencyState::Queued;
    c.permits_in_use = 1;
    admit(&mut c, 1);

    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.mandatory, vec![0, 1]);
    assert_eq!(
        batch.victims,
        vec![3, 2, 4, 5],
        "Held goes after everything"
    );
    accept_evicting(&mut c, &batch, vec![3, 2, 4, 5]);
    assert_eq!(c.state(5), Some(ClusterResidencyState::Absent));
    assert!(!c.is_targeted(5));

    // The frame still draws it. It counts a miss, and a hold reads nothing.
    frame(&mut c, &set, 0, FRUSTUM, &[5], 0.3);
    assert!(!c.is_targeted(5));
    assert!(c.take_requests().unwrap().iter().all(|r| r.cluster_id != 5));
}

#[test]
fn a_held_resident_has_no_settle_or_permit_claim_and_no_departure_takes_it() {
    let mut c = level();
    let set = corridor();
    frame(&mut c, &set, 0, PORTAL, &[0, 5], 0.0);
    for id in [0, 1, 2, 3, 5] {
        mark_sampleable(&mut c, id);
    }
    frame(&mut c, &set, 0, FRUSTUM, &[5], 0.1);
    frame(&mut c, &set, 0, FRUSTUM, &[5], 50.0);
    assert_eq!(c.states[5].class, Some(TargetClass::Held));
    assert_eq!(c.settle_set_targets(), Some(2), "lead 0 and 1, not Held 5");
    assert_eq!(c.request_permit_limit(5), MAX_OPTIONAL_STREAM_PERMITS);

    let batch = c.take_async_drain_batch().unwrap();
    assert!(batch.evictions.is_empty(), "residents stay");
}

// Regression: a read completing while drawn on a held frame was promoted to
// Visible, so it could evict and grow for work the bound never priced.
#[test]
fn a_band_read_completing_while_drawn_on_a_held_frame_cannot_grow_the_pool() {
    let mut c = level();
    let set = corridor();
    frame(&mut c, &set, 0, PORTAL, &[0], 0.0);
    for id in [0, 1, 3] {
        mark_sampleable(&mut c, id);
    }
    assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 2);

    frame(&mut c, &set, 0, FRUSTUM, &[2], 0.1);
    assert_eq!(c.states[2].class, Some(TargetClass::Band));
    admit(&mut c, 2);
    frame(&mut c, &set, 0, FRUSTUM, &[2], 0.2);
    assert_eq!(c.states[2].class, Some(TargetClass::Band));
    assert!(
        !c.mandatory_work_ready(),
        "only mandatory work grows a pool"
    );

    refuse_pool(&mut c, 2);
    assert_eq!(c.state(2), Some(ClusterResidencyState::Absent));
    assert!(!c.is_targeted(2) && c.states[2].suppressed);
    assert!(c.states[2].failure.is_none());
}

// With no reach class to keep, a read in flight lands Held: no longer wanted
// enough to displace anything, and refused as optional work is.
#[test]
fn a_read_with_no_reach_class_lands_held_and_is_refused_like_optional_work() {
    let mut c = level();
    let set = corridor();
    frame(&mut c, &set, 0, PORTAL, &[5], 0.0);
    for id in [0, 1, 2, 3] {
        mark_sampleable(&mut c, id);
    }
    assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 5);

    frame(&mut c, &set, 0, FRUSTUM, &[5], 0.1);
    assert_eq!(c.states[5].class, Some(TargetClass::Held));
    admit(&mut c, 5);
    assert!(!c.mandatory_work_ready());

    refuse_pool(&mut c, 5);
    assert_eq!(c.state(5), Some(ClusterResidencyState::Absent));
    assert!(!c.is_targeted(5) && c.states[5].suppressed);
    assert_eq!(c.permits_in_use(), 0);
}

#[test]
fn paused_hysteresis_residents_are_victims_last_and_do_not_age() {
    let mut c = level();
    let set = corridor();
    frame(&mut c, &set, 0, PORTAL, &[0, 4], 0.0);
    for id in [0, 2, 3, 4] {
        mark_sampleable(&mut c, id);
    }
    frame(&mut c, &set, 0, PORTAL, &[0], 0.1);
    assert_eq!(c.states[4].class, Some(TargetClass::Hysteresis));

    // Well past the window, but the run's clock is paused.
    frame(&mut c, &set, 0, FRUSTUM, &[0], 0.2);
    frame(&mut c, &set, 0, FRUSTUM, &[0], 30.0);
    assert_eq!(c.states[4].class, Some(TargetClass::Hysteresis));
    c.states[1].state = ClusterResidencyState::Queued;
    c.permits_in_use = 1;
    admit(&mut c, 1);

    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.victims, vec![3, 2, 4]);
}

#[test]
fn a_resident_only_the_mirror_draws_is_no_victim() {
    let mut c = level();
    let set = corridor();
    let targeted = VisibleCells::Culled(vec![0]);
    let drawn = VisibleCells::Culled(vec![0, 2]);
    let stage = CellDemand::new(set.max_lead);
    let reach: DemandFrame<'_> = stage.frame(&set, 0, PORTAL, &drawn);
    c.update_targets(&targeted, Some(reach), 0.0).unwrap();
    assert_eq!(c.states[2].class, Some(TargetClass::Band));
    queue_ready(&mut c, 0);
    for id in [2, 3] {
        mark_sampleable(&mut c, id);
    }

    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.victims, vec![3], "the frame still draws band 2");
    accept_evicting(&mut c, &batch, vec![3]);

    // Next frame the mirror is gone, and so is the protection.
    frame(&mut c, &set, 0, PORTAL, &[0], 0.1);
    c.states[1].state = ClusterResidencyState::Queued;
    c.permits_in_use = 1;
    admit(&mut c, 1);
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.victims, vec![2]);
}

#[test]
fn a_named_victim_the_renderer_keeps_stays_resident_and_targeted() {
    let mut c = level();
    let set = corridor();
    frame(&mut c, &set, 0, PORTAL, &[0], 0.0);
    queue_ready(&mut c, 0);
    for id in [2, 3] {
        mark_sampleable(&mut c, id);
    }
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.victims, vec![3, 2]);
    accept_evicting(&mut c, &batch, Vec::new());

    for id in [2, 3] {
        assert_eq!(c.state(id), Some(ClusterResidencyState::Sampleable));
        assert!(c.is_targeted(id) && !c.states[id as usize].suppressed);
    }
    assert!(c.in_drain_victims.is_empty());
}

#[test]
fn an_evicted_hysteresis_victim_is_not_read_back_when_its_timer_had_time_left() {
    let mut c = level();
    let set = corridor();
    frame(&mut c, &set, 0, PORTAL, &[0, 4], 0.0);
    mark_sampleable(&mut c, 4);
    frame(&mut c, &set, 0, PORTAL, &[0], 0.1);
    assert_eq!(c.states[4].class, Some(TargetClass::Hysteresis));
    assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 0);
    admit(&mut c, 0);
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.victims, vec![4]);
    accept_evicting(&mut c, &batch, vec![4]);
    assert!(!c.is_targeted(4));

    // A new view lifts suppression; its retention was what the room cost.
    frame(&mut c, &set, 0, PORTAL, &[0, 1], 0.2);
    assert!(!c.is_targeted(4));
    assert!(
        c.take_requests()
            .unwrap()
            .iter()
            .all(|request| request.cluster_id != 4)
    );
}

#[test]
fn a_pool_refused_install_releases_its_payload_and_yields_until_the_horizon_changes() {
    let mut c = level();
    let set = corridor();
    frame(&mut c, &set, 0, PORTAL, &[0], 0.0);
    for id in [0, 1] {
        mark_sampleable(&mut c, id);
    }
    assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 2);
    admit(&mut c, 2);
    assert_eq!(c.cpu().ready.current_bytes, 4);

    refuse_pool(&mut c, 2);
    assert_eq!(c.state(2), Some(ClusterResidencyState::Absent));
    assert_eq!(c.permits_in_use(), 0);
    assert_eq!(c.cpu().ready.current_bytes, 0);
    assert!(!c.is_targeted(2) && c.states[2].suppressed);
    assert!(
        c.states[2].failure.is_none(),
        "no failure: the pool was full"
    );
    assert_eq!(c.counters().installs, 0);

    frame(&mut c, &set, 0, PORTAL, &[0], 1.0);
    assert!(!c.is_targeted(2));
    assert!(c.take_requests().unwrap().iter().all(|r| r.cluster_id != 2));
    let _ = c.take_async_drain_batch().unwrap();
    c.apply_drain_outcome(ShDrainOutcome::default()).unwrap();

    frame(&mut c, &set, 0, PORTAL, &[0, 7], 2.0);
    assert!(c.is_targeted(2), "a new view asks the renderer again");
}

#[test]
fn an_outcome_is_rejected_whole_if_it_evicts_what_no_list_named_or_misplaces_a_refusal() {
    let mut c = level();
    let set = corridor();
    frame(&mut c, &set, 0, PORTAL, &[0], 0.0);
    queue_ready(&mut c, 0);
    for id in [2, 3] {
        mark_sampleable(&mut c, id);
    }
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.victims, vec![3, 2]);
    for outcome in [
        ShDrainOutcome {
            accepted: vec![0],
            evicted: vec![1],
            ..ShDrainOutcome::default()
        },
        ShDrainOutcome {
            pool_refused: vec![2],
            accepted: vec![0],
            ..ShDrainOutcome::default()
        },
        ShDrainOutcome {
            pool_refused: vec![0],
            accepted: vec![0],
            ..ShDrainOutcome::default()
        },
    ] {
        assert!(matches!(
            c.apply_drain_outcome(outcome),
            Err(ShResidencyControllerError::InvalidDrainOutcome(_))
        ));
    }
    assert_eq!(c.state(3), Some(ClusterResidencyState::Sampleable));
    assert!(c.is_targeted(3) && !c.states[3].suppressed);
    assert_eq!(c.permits_in_use(), 1);
}

// AC 10 (R3) under a capacity victim: the renderer's half drops such a read
// too (`read_finishing_for_an_evicted_victim_is_dropped`). A resident has no
// read in flight, so the read is the one a new view started after the
// eviction, finishing once that view has moved on.
#[test]
fn read_finishing_for_a_capacity_victim_is_dropped() {
    let mut c = level();
    let set = corridor();
    frame(&mut c, &set, 0, PORTAL, &[0], 0.0);
    queue_ready(&mut c, 0);
    mark_sampleable(&mut c, 3);
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.victims, vec![3]);
    accept_evicting(&mut c, &batch, vec![3]);
    assert_eq!(c.counters().evictions, 1);

    frame(&mut c, &set, 0, PORTAL, &[0, 7], 0.1);
    let requests = c.take_requests().unwrap();
    assert!(requests.iter().any(|request| request.cluster_id == 3));
    let permits = c.permits_in_use();

    // The camera moves on with the read still running.
    frame(&mut c, &set, 4, PORTAL, &[4], 0.2);
    frame(&mut c, &set, 4, PORTAL, &[4], 3.0);
    assert!(!c.is_targeted(3));
    assert_eq!(
        c.admit_prepared(prepared(&c, 3)).unwrap(),
        ShDrainAdmission::DroppedNotTargeted
    );
    assert_eq!(c.state(3), Some(ClusterResidencyState::Absent));
    assert_eq!(c.permits_in_use(), permits - 1);
    assert_eq!(c.cpu().ready.current_bytes, 0);
    let batch = c.take_async_drain_batch().unwrap();
    assert!(batch.ready.iter().all(|p| p.chunk.cluster_id != 3));
}

// AC 7 (R10), app half: the renderer's half is
// `reload_mid_flight_leaves_nothing_from_the_old_level`.
#[test]
fn reload_mid_flight_leaves_no_pending_read_from_the_old_level() {
    let mut old = ShResidencyController::for_test(
        topology((0..8).collect(), vec![Vec::new(); 8]),
        &FixedGenerationClock::new(1),
    )
    .unwrap();
    let set = corridor();
    frame(&mut old, &set, 0, PORTAL, &[0], 0.0);
    let in_flight = old.take_requests().unwrap();
    assert!(!in_flight.is_empty());
    let completion = prepared(&old, in_flight[0].cluster_id);

    let mut reloaded = ShResidencyController::for_test(
        topology((0..8).collect(), vec![Vec::new(); 8]),
        &FixedGenerationClock::new(2),
    )
    .unwrap();
    frame(&mut reloaded, &set, 0, PORTAL, &[0], 0.0);
    let fresh = reloaded.take_next_request().unwrap().unwrap();
    assert!(!reloaded.matches_completion_identity(in_flight[0]));

    assert_eq!(
        reloaded.admit_prepared(completion).unwrap(),
        ShDrainAdmission::DroppedStale
    );
    assert_eq!(reloaded.permits_in_use(), 1, "only the new read holds one");
    assert_eq!(reloaded.cpu().ready.current_bytes, 0);
    assert_eq!(
        reloaded.state(fresh.cluster_id),
        Some(ClusterResidencyState::Queued)
    );
    assert!(reloaded.take_async_drain_batch().unwrap().ready.is_empty());
}

/// The renderer's report that its pools hold the whole map, on an empty drain.
fn report_whole_map_pool(c: &mut ShResidencyController) {
    c.take_async_drain_batch().unwrap();
    c.apply_drain_outcome(ShDrainOutcome {
        pool_holds_whole_map: true,
        ..ShDrainOutcome::default()
    })
    .unwrap();
}

/// Cluster 5 drawn, then gone past its hysteresis: a resident in no target.
fn depart_cluster_five(c: &mut ShResidencyController, set: &CellResidencySetSection) {
    frame(c, set, 0, PORTAL, &[0, 5], 0.0);
    for id in [0, 1, 2, 3, 5] {
        mark_sampleable(c, id);
    }
    frame(c, set, 0, PORTAL, &[0], 0.1);
    frame(c, set, 0, PORTAL, &[0], 0.1 + HYSTERESIS_SECONDS);
    assert!(!c.targets.contains(&5));
}

// The contract this replaces, kept as the control: below the map, a departed
// resident is evicted even with room to spare.
#[test]
fn a_pool_short_of_the_whole_map_still_evicts_departures() {
    let mut c = level();
    let set = corridor();
    depart_cluster_five(&mut c, &set);
    assert_eq!(c.take_async_drain_batch().unwrap().evictions, [5]);
}

#[test]
fn a_pool_holding_the_whole_map_keeps_departed_residents() {
    let mut c = level();
    let set = corridor();
    report_whole_map_pool(&mut c);
    depart_cluster_five(&mut c, &set);

    let batch = c.take_async_drain_batch().unwrap();
    assert!(batch.evictions.is_empty());
    assert_eq!(c.states[5].state, ClusterResidencyState::Sampleable);
    c.apply_drain_outcome(ShDrainOutcome {
        pool_holds_whole_map: true,
        ..ShDrainOutcome::default()
    })
    .unwrap();

    // Coming back costs no read: this is what the map saves in re-reads.
    let before = c.counters().rereads;
    frame(&mut c, &set, 0, PORTAL, &[0, 5], 3.0);
    assert!(c.take_next_request().unwrap().is_none());
    assert_eq!(c.counters().rereads, before);
}

#[test]
fn a_pool_holding_the_whole_map_names_no_capacity_victim() {
    let mut c = level();
    let set = corridor();
    report_whole_map_pool(&mut c);
    frame(&mut c, &set, 0, PORTAL, &[0], 0.0);
    for id in 1..4 {
        mark_sampleable(&mut c, id);
    }
    assert_eq!(c.take_next_request().unwrap().unwrap().cluster_id, 0);
    admit(&mut c, 0);

    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.mandatory, [0, 1]);
    assert!(batch.victims.is_empty() && batch.evictions.is_empty());
}

// The fact rides each outcome, so nothing latches it. A pool at the cap that
// has released a range reports false, and its victims and departures still
// flow: they are what recover fragmentation.
#[test]
fn the_whole_map_fact_is_read_from_each_outcome() {
    let mut c = level();
    let set = corridor();
    report_whole_map_pool(&mut c);
    depart_cluster_five(&mut c, &set);
    assert!(c.take_async_drain_batch().unwrap().evictions.is_empty());
    c.apply_drain_outcome(ShDrainOutcome::default()).unwrap();

    c.states[0].state = ClusterResidencyState::Queued;
    c.permits_in_use = 1;
    admit(&mut c, 0);
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.evictions, [5]);
    assert_eq!(batch.victims, [3, 2]);
}
