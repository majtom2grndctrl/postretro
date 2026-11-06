// The hold: demand off the portal path, full-demand frames, and paused hysteresis.
// See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use std::collections::BTreeSet;

use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_visibility::{VisibilityPath, VisibleCells};

use super::super::topology::SeamPortalEndpoint;
use super::tests::{controller, hinted_topology, mark_sampleable, prepared, topology};
use super::*;
use crate::lightmap_streaming::test_fixtures::{PORTAL, residency_set};
use crate::streaming::cell_demand::CellDemand;

const FALLBACKS: [VisibilityPath; 4] = [
    VisibilityPath::PortalStepLimitFallback {
        considered: 0,
        accepted: 0,
    },
    VisibilityPath::NoPortalsFallback,
    VisibilityPath::SolidCellFallback,
    VisibilityPath::ExteriorCellFallback,
];
const SOLID: VisibilityPath = VisibilityPath::SolidCellFallback;

/// Eight clusters, one cell each. From camera cell 0 the reach is 0 (0 m), 1
/// (4 m) and 2 (20 m, the band); from camera cell 6 it is 6 (0 m) and 7 (4 m).
/// Cell 5 has no baked set. Clusters 3 and 4 are outside every reach.
fn level() -> (ShResidencyController, CellResidencySetSection) {
    let set = residency_set(
        8,
        &[(0, 0, 0), (0, 1, 4), (0, 2, 20), (6, 6, 0), (6, 7, 4)],
        32,
    );
    let controller = controller(topology((0..8).collect(), vec![Vec::new(); 8]));
    (controller, set)
}

/// One frame from `camera_cell` on `path`, drawing and targeting `drawn`.
fn frame(
    controller: &mut ShResidencyController,
    set: &CellResidencySetSection,
    camera_cell: u32,
    path: VisibilityPath,
    drawn: &[u32],
    now: f64,
) {
    let cells = VisibleCells::Culled(drawn.to_vec());
    let stage = CellDemand::new(set.max_lead);
    controller
        .update_targets(
            &cells,
            Some(stage.frame(set, camera_cell, path, &cells)),
            now,
        )
        .unwrap();
}

fn requested(controller: &mut ShResidencyController) -> BTreeSet<u32> {
    controller
        .take_requests()
        .unwrap()
        .iter()
        .map(|request| request.cluster_id)
        .collect()
}

/// Every requested read lands, installs and composes.
fn stream_until_idle(controller: &mut ShResidencyController) {
    loop {
        let requests = requested(controller);
        if requests.is_empty() {
            return;
        }
        for cluster_id in requests {
            assert_eq!(
                controller
                    .admit_prepared(prepared(controller, cluster_id))
                    .unwrap(),
                ShDrainAdmission::Ready
            );
        }
        let batch = controller.take_async_drain_batch().unwrap();
        let accepted = batch.ready.iter().map(|p| p.chunk.cluster_id).collect();
        controller
            .apply_drain_outcome(ShDrainOutcome {
                accepted,
                ..ShDrainOutcome::default()
            })
            .unwrap();
        controller.promote_composed_clusters();
    }
}

/// The drain's eviction requests, applied.
fn drain_evictions(controller: &mut ShResidencyController) -> Vec<u32> {
    let batch = controller.take_async_drain_batch().unwrap();
    let evicted = batch.evictions.clone();
    controller
        .apply_drain_outcome(ShDrainOutcome {
            evicted: batch.evictions,
            ..ShDrainOutcome::default()
        })
        .unwrap();
    evicted
}

fn class(controller: &ShResidencyController, cluster_id: u32) -> Option<TargetClass> {
    controller.states[cluster_id as usize].class
}

fn assert_buckets_sum_to_misses(controller: &ShResidencyController) {
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

// AC 19.
#[test]
fn each_fallback_path_holds_and_counts_misses() {
    for path in FALLBACKS {
        let (mut c, set) = level();
        frame(&mut c, &set, 0, PORTAL, &[0, 3], 0.0);
        stream_until_idle(&mut c);
        assert_eq!(c.state(3), Some(ClusterResidencyState::Sampleable));
        let misses = c.counters().misses;
        let outside_reach = c.counters().miss_buckets.outside_reach;

        // Resident 3 and absent 4 are both drawn, outside the reach.
        frame(&mut c, &set, 0, path, &[0, 3, 4], 1.0);
        assert_eq!(class(&c, 3), Some(TargetClass::Visible), "{path:?}");
        assert!(!c.is_targeted(4), "{path:?}");
        assert_eq!(requested(&mut c), BTreeSet::new(), "{path:?}");
        assert!(drain_evictions(&mut c).is_empty(), "{path:?}");
        assert_eq!(c.state(3), Some(ClusterResidencyState::Sampleable));
        assert_eq!(c.state(4), Some(ClusterResidencyState::Absent));
        assert_eq!(c.counters().misses, misses + 1, "{path:?}: cluster 4 alone");
        assert_eq!(
            c.counters().miss_buckets.outside_reach,
            outside_reach + 1,
            "{path:?}"
        );
        assert_buckets_sum_to_misses(&c);

        // The episode counts once however long the run draws it.
        frame(&mut c, &set, 0, path, &[0, 3, 4], 1.1);
        assert_eq!(c.counters().misses, misses + 1, "{path:?}");
    }
}

// AC 19: a cluster already on its way stays protected; losing the target
// would throw away the read.
#[test]
fn held_frame_protects_a_drawn_cluster_on_its_way() {
    for state in [
        ClusterResidencyState::Queued,
        ClusterResidencyState::Ready,
        ClusterResidencyState::InstalledUncomposed,
    ] {
        let (mut c, set) = level();
        frame(&mut c, &set, 0, PORTAL, &[3], 0.0);
        assert!(requested(&mut c).contains(&3));
        if state != ClusterResidencyState::Queued {
            assert_eq!(
                c.admit_prepared(prepared(&c, 3)).unwrap(),
                ShDrainAdmission::Ready
            );
        }
        if state == ClusterResidencyState::InstalledUncomposed {
            let batch = c.take_async_drain_batch().unwrap();
            let accepted = batch.ready.iter().map(|p| p.chunk.cluster_id).collect();
            c.apply_drain_outcome(ShDrainOutcome {
                accepted,
                ..ShDrainOutcome::default()
            })
            .unwrap();
        }
        assert_eq!(c.state(3), Some(state));

        frame(&mut c, &set, 0, SOLID, &[3], 1.0);
        assert_eq!(class(&c, 3), Some(TargetClass::Visible), "{state:?}");
        assert_eq!(c.state(3), Some(state), "{state:?}");
    }
}

// A read cancelled after the frame's targets were set lands back at Absent
// under a Visible target. It must not read again for that drawn cluster.
#[test]
fn held_frame_does_not_reread_a_drawn_cluster_its_read_was_cancelled_for() {
    let (mut c, set) = level();
    frame(&mut c, &set, 0, PORTAL, &[3], 0.0);
    let request = c
        .take_requests()
        .unwrap()
        .into_iter()
        .find(|request| request.cluster_id == 3)
        .unwrap();
    frame(&mut c, &set, 0, SOLID, &[3], 1.0);
    assert_eq!(class(&c, 3), Some(TargetClass::Visible));
    c.admit_cancelled_request(request).unwrap();
    assert_eq!(c.state(3), Some(ClusterResidencyState::Absent));
    assert!(!requested(&mut c).contains(&3));
}

// The mirror's cells are drawn but not targeted on a fallback. Regression: the
// spike handed the targeter an empty set, so its misses went uncounted.
#[test]
fn mirror_cells_on_a_fallback_count_misses_and_add_no_reads() {
    let (mut c, set) = level();
    let stage = CellDemand::new(set.max_lead);
    let targeted = VisibleCells::Culled(vec![0]);
    let drawn = VisibleCells::Culled(vec![0, 3, 4]);
    frame(&mut c, &set, 0, PORTAL, &[0, 3], 0.0);
    stream_until_idle(&mut c);
    let outside_reach = c.counters().miss_buckets.outside_reach;

    c.update_targets(&targeted, Some(stage.frame(&set, 0, SOLID, &drawn)), 1.0)
        .unwrap();
    assert_eq!(
        c.counters().miss_buckets.outside_reach,
        outside_reach + 1,
        "cluster 4"
    );
    assert_eq!(
        class(&c, 3),
        Some(TargetClass::Visible),
        "resident 3 is drawn, so it is not freed under the frame"
    );
    assert!(!c.is_targeted(4));
    assert!(requested(&mut c).is_empty());
    assert_buckets_sum_to_misses(&c);
}

// On the portal path too, misses read the drawn set: a tripped mirror leaves
// `visible` short of it.
#[test]
fn portal_frame_counts_misses_from_the_drawn_set() {
    let (mut c, set) = level();
    let stage = CellDemand::new(set.max_lead);
    let targeted = VisibleCells::Culled(vec![0]);
    let drawn = VisibleCells::Culled(vec![0, 4]);
    c.update_targets(&targeted, Some(stage.frame(&set, 0, PORTAL, &drawn)), 0.0)
        .unwrap();
    assert_eq!(c.counters().misses, 2);
    assert!(c.is_targeted(0));
    assert!(!c.is_targeted(4), "targeting reads `visible`");
}

// AC 20.
#[test]
fn no_id51_keeps_visible_demand_on_every_path() {
    let (mut c, _) = level();
    // A level with no usable id 51 hands no frame, so no path can hold.
    c.update_targets(&VisibleCells::Culled(vec![3]), None, 0.0)
        .unwrap();
    assert_eq!(class(&c, 3), Some(TargetClass::Visible));
    assert_eq!(requested(&mut c), BTreeSet::from([3]));
    // The camera moves on: new cells load, and the old ones age out.
    c.update_targets(&VisibleCells::Culled(vec![4]), None, 1.0)
        .unwrap();
    assert_eq!(class(&c, 4), Some(TargetClass::Visible));
    assert_eq!(class(&c, 3), Some(TargetClass::Hysteresis));
    assert_eq!(requested(&mut c), BTreeSet::from([4]));
    c.update_targets(&VisibleCells::Culled(vec![4]), None, 3.1)
        .unwrap();
    assert_eq!(class(&c, 3), None);
}

// AC 20: an id 51 for another cell map is no id 51.
#[test]
fn an_unusable_id51_keeps_visible_demand_on_a_fallback() {
    let (mut c, _) = level();
    let foreign = residency_set(3, &[(0, 0, 0)], 32);
    frame(&mut c, &foreign, 0, SOLID, &[3], 0.0);
    assert_eq!(class(&c, 3), Some(TargetClass::Visible));
    assert_eq!(requested(&mut c), BTreeSet::from([3]));
}

// AC 21.
#[test]
fn portal_path_keeps_drawn_clusters_visible() {
    let (mut c, set) = level();
    frame(&mut c, &set, 0, PORTAL, &[3, 4], 0.0);
    assert_eq!(class(&c, 3), Some(TargetClass::Visible));
    assert_eq!(class(&c, 4), Some(TargetClass::Visible));
    let requests = requested(&mut c);
    assert!(requests.contains(&3) && requests.contains(&4));
}

// AC 22.
#[test]
fn leaving_fallback_restores_demand_on_that_frame() {
    for path in FALLBACKS {
        let (mut c, set) = level();
        frame(&mut c, &set, 0, path, &[4], 0.0);
        assert!(!c.is_targeted(4), "{path:?}");
        frame(&mut c, &set, 0, PORTAL, &[4], 0.1);
        assert_eq!(class(&c, 4), Some(TargetClass::Visible), "{path:?}");
        assert!(requested(&mut c).contains(&4), "{path:?}");
    }
}

// AC 23: Settling and capture's preload share this mode.
#[test]
fn settling_on_fallback_keeps_full_demand() {
    for path in FALLBACKS {
        let (mut c, set) = level();
        c.set_full_demand(true);
        frame(&mut c, &set, 0, path, &[3, 4], 0.0);
        assert_eq!(class(&c, 3), Some(TargetClass::Visible), "{path:?}");
        assert_eq!(class(&c, 4), Some(TargetClass::Visible), "{path:?}");
        let requests = requested(&mut c);
        assert!(requests.contains(&3) && requests.contains(&4), "{path:?}");
        assert_eq!(c.unsettled_targets(), Some(4), "visible 3, 4, lead 0, 1");
        assert_eq!(c.counters().misses, 0, "{path:?}: nothing is presented");
    }
}

// AC 24 (R9).
#[test]
fn settling_end_on_fallback_keeps_residents() {
    for path in FALLBACKS {
        let (mut c, set) = level();
        c.set_full_demand(true);
        frame(&mut c, &set, 0, path, &[3, 4], 0.0);
        stream_until_idle(&mut c);
        let resident: Vec<_> = (0..5).map(|id| c.state(id)).collect();
        assert!(
            resident
                .iter()
                .all(|&state| state == Some(ClusterResidencyState::Sampleable))
        );

        // The reveal frame's pose turned: 4 is no longer drawn.
        c.set_full_demand(false);
        frame(&mut c, &set, 0, path, &[3], 5.0);
        assert_eq!(c.counters().misses, 0, "{path:?}");
        assert_eq!(class(&c, 3), Some(TargetClass::Visible), "{path:?}");
        assert_eq!(class(&c, 4), Some(TargetClass::Hysteresis), "{path:?}");
        assert!(drain_evictions(&mut c).is_empty(), "{path:?}");
        frame(&mut c, &set, 0, path, &[3], 50.0);
        assert!(drain_evictions(&mut c).is_empty(), "{path:?}");
        assert_eq!((0..5).map(|id| c.state(id)).collect::<Vec<_>>(), resident);
    }
}

// AC 25 (R7).
#[test]
fn long_fallback_run_ages_out_nothing() {
    for path in FALLBACKS {
        let (mut c, set) = level();
        frame(&mut c, &set, 0, PORTAL, &[3, 4], 0.0);
        stream_until_idle(&mut c);

        // Cluster 3 departs half a second before the run; 4 departs inside it.
        frame(&mut c, &set, 0, PORTAL, &[4], 1.0);
        frame(&mut c, &set, 0, PORTAL, &[4], 1.5);
        frame(&mut c, &set, 0, path, &[4], 1.6);
        frame(&mut c, &set, 0, path, &[], 2.0);
        for now in [10.0, 100.0, 1000.0] {
            frame(&mut c, &set, 0, path, &[], now);
            assert_eq!(class(&c, 3), Some(TargetClass::Hysteresis), "{path:?}");
            assert_eq!(class(&c, 4), Some(TargetClass::Hysteresis), "{path:?}");
            assert!(drain_evictions(&mut c).is_empty(), "{path:?}");
        }

        // First portal frame: every timer resumes with what it had left.
        frame(&mut c, &set, 0, PORTAL, &[], 1000.5);
        assert!(drain_evictions(&mut c).is_empty(), "{path:?}");
        frame(&mut c, &set, 0, PORTAL, &[], 1001.9);
        assert_eq!(class(&c, 3), Some(TargetClass::Hysteresis), "{path:?}");
        assert!(drain_evictions(&mut c).is_empty(), "{path:?}");
        // 3 had 1.5 s left, 4 the whole 2 s.
        frame(&mut c, &set, 0, PORTAL, &[], 1002.1);
        assert_eq!(class(&c, 3), None, "{path:?}");
        assert_eq!(class(&c, 4), Some(TargetClass::Hysteresis), "{path:?}");
        assert_eq!(drain_evictions(&mut c), vec![3], "{path:?}");
        frame(&mut c, &set, 0, PORTAL, &[], 1002.6);
        assert_eq!(drain_evictions(&mut c), vec![4], "{path:?}");
    }
}

// R7: portal time ages hysteresis wherever it falls between runs.
#[test]
fn portal_frames_between_runs_age_hysteresis_only_by_their_own_time() {
    let (mut c, set) = level();
    frame(&mut c, &set, 0, PORTAL, &[3], 0.0);
    stream_until_idle(&mut c);
    frame(&mut c, &set, 0, PORTAL, &[], 1.0);
    frame(&mut c, &set, 0, SOLID, &[], 50.0);
    frame(&mut c, &set, 0, PORTAL, &[], 60.0);
    frame(&mut c, &set, 0, PORTAL, &[], 60.9);
    frame(&mut c, &set, 0, SOLID, &[], 70.0);
    frame(&mut c, &set, 0, PORTAL, &[], 80.0);
    assert_eq!(class(&c, 3), Some(TargetClass::Hysteresis));
    frame(&mut c, &set, 0, PORTAL, &[], 81.2);
    assert_eq!(class(&c, 3), None, "0.9 s between the runs, 1.2 s after");
}

// R7: an absent departing cluster's timer pauses too, and a held frame spends
// no read on it.
#[test]
fn held_frame_reads_no_absent_hysteresis_cluster() {
    let (mut c, set) = level();
    frame(&mut c, &set, 0, PORTAL, &[3], 0.0);
    frame(&mut c, &set, 0, SOLID, &[], 0.1);
    assert!(!c.is_targeted(3));
    assert!(!requested(&mut c).contains(&3));
    assert_eq!(c.state(3), Some(ClusterResidencyState::Absent));

    frame(&mut c, &set, 0, PORTAL, &[], 0.2);
    assert_eq!(
        class(&c, 3),
        Some(TargetClass::Hysteresis),
        "its timer waited out the run"
    );
}

// AC 26 (R8).
#[test]
fn teleport_during_hold_follows_the_new_cell() {
    for path in FALLBACKS {
        let (mut c, set) = level();
        frame(&mut c, &set, 0, PORTAL, &[0], 0.0);
        stream_until_idle(&mut c);

        // Teleported mid-run, and drawing neither arrival cluster.
        frame(&mut c, &set, 6, path, &[3], 1.0);
        assert_eq!(class(&c, 6), Some(TargetClass::Lead), "{path:?}");
        assert_eq!(class(&c, 7), Some(TargetClass::Lead), "{path:?}");
        assert_eq!(
            class(&c, 0),
            Some(TargetClass::Hysteresis),
            "{path:?}: the departed reach stays resident through the hold"
        );
        assert_eq!(requested(&mut c), BTreeSet::from([6, 7]), "{path:?}");
        for cluster_id in [6, 7] {
            assert_eq!(
                c.admit_prepared(prepared(&c, cluster_id)).unwrap(),
                ShDrainAdmission::Ready
            );
        }
        let batch = c.take_async_drain_batch().unwrap();
        let accepted = batch.ready.iter().map(|p| p.chunk.cluster_id).collect();
        c.apply_drain_outcome(ShDrainOutcome {
            accepted,
            ..ShDrainOutcome::default()
        })
        .unwrap();

        frame(&mut c, &set, 6, PORTAL, &[6], 2.0);
        c.promote_composed_clusters();
        for cluster_id in [6, 7] {
            assert_eq!(
                c.state(cluster_id),
                Some(ClusterResidencyState::Sampleable),
                "{path:?}"
            );
        }
    }
}

// A cell with no baked set names nowhere to go, so the current reach stands.
#[test]
fn solid_cell_with_no_baked_set_keeps_the_reach_through_a_hold() {
    let (mut c, set) = level();
    frame(&mut c, &set, 0, PORTAL, &[0], 0.0);
    stream_until_idle(&mut c);
    frame(&mut c, &set, 5, SOLID, &[0], 1.0);
    frame(&mut c, &set, 5, SOLID, &[0], 2.0);
    assert_eq!(class(&c, 1), Some(TargetClass::Lead));
    assert_eq!(class(&c, 2), Some(TargetClass::Band));
    assert!(drain_evictions(&mut c).is_empty());
}

// `DrawAll` comes only from the empty world, which draws nothing.
#[test]
fn empty_world_frame_never_reads_as_every_cluster() {
    let (mut c, set) = level();
    frame(&mut c, &set, 0, PORTAL, &[3], 0.0);
    stream_until_idle(&mut c);
    let misses = c.counters().misses;
    let draw_all = VisibleCells::DrawAll;
    let stage = CellDemand::new(set.max_lead);
    c.update_targets(
        &draw_all,
        Some(stage.frame(&set, 0, VisibilityPath::EmptyWorldFallback, &draw_all)),
        1.0,
    )
    .unwrap();
    assert!(!c.is_targeted(4));
    assert!(requested(&mut c).is_empty());
    assert_eq!(c.counters().misses, misses, "it draws no cell");
    assert_eq!(class(&c, 3), Some(TargetClass::Hysteresis));
    assert_eq!(class(&c, 1), Some(TargetClass::Lead), "the reach stands");
}

// A miss episode spans the path change: the drawn set is one source on both.
#[test]
fn drawn_cluster_misses_once_across_a_change_of_path() {
    let (mut c, set) = level();
    frame(&mut c, &set, 0, SOLID, &[4], 0.0);
    frame(&mut c, &set, 0, PORTAL, &[4], 0.1);
    frame(&mut c, &set, 0, SOLID, &[4], 0.2);
    assert_eq!(c.counters().misses, 1);
    assert_buckets_sum_to_misses(&c);
    frame(&mut c, &set, 0, SOLID, &[], 0.3);
    frame(&mut c, &set, 0, SOLID, &[4], 0.4);
    assert_eq!(c.counters().misses, 2, "a fresh episode");
}

// A frustum fallback draws every seam's near side: warming from it would read
// the far sides. A cluster the seam already warmed has nothing else to keep it.
#[test]
fn hold_warms_no_new_seam_and_keeps_the_warm_set() {
    let seams = vec![
        SeamPortalEndpoint {
            portal_id: 0,
            front_cluster_id: 3,
            back_cluster_id: 4,
        },
        SeamPortalEndpoint {
            portal_id: 1,
            front_cluster_id: 5,
            back_cluster_id: 6,
        },
    ];
    let mut c = controller(hinted_topology(
        (0..8).collect(),
        vec![Vec::new(); 8],
        seams,
        BTreeSet::new(),
        Vec::new(),
    ));
    let set = residency_set(8, &[(0, 0, 0)], 32);
    frame(&mut c, &set, 0, PORTAL, &[3], 0.0);
    stream_until_idle(&mut c);
    assert_eq!(class(&c, 4), Some(TargetClass::SeamWarm));
    assert_eq!(c.state(5), Some(ClusterResidencyState::Absent));
    mark_sampleable(&mut c, 5);

    frame(&mut c, &set, 0, SOLID, &[5], 1.0);
    assert_eq!(
        class(&c, 4),
        Some(TargetClass::SeamWarm),
        "the seam's near side left the view, and the hold keeps its far side"
    );
    assert!(!c.is_targeted(6), "drawing 5 warms nothing on a held frame");
    assert!(requested(&mut c).is_empty());
    assert!(drain_evictions(&mut c).is_empty());
}

// AC 9, app half (R2).
#[test]
fn evict_and_rerequest_counts_references_once() {
    let (mut c, set) = level();
    frame(&mut c, &set, 0, PORTAL, &[3], 0.0);
    stream_until_idle(&mut c);

    frame(&mut c, &set, 0, PORTAL, &[], 0.1);
    frame(&mut c, &set, 0, PORTAL, &[], 2.2);
    assert_eq!(drain_evictions(&mut c), vec![3]);

    // Back in view on the frame after its eviction landed.
    frame(&mut c, &set, 0, PORTAL, &[3], 2.3);
    assert_eq!(requested(&mut c), BTreeSet::from([3]));
    assert_eq!(c.counters().rereads, 1);
    assert_eq!(c.counters().evictions, 1);
    assert_eq!(
        c.admit_prepared(prepared(&c, 3)).unwrap(),
        ShDrainAdmission::Ready
    );
    let batch = c.take_async_drain_batch().unwrap();
    assert_eq!(batch.ready.len(), 1);
    c.apply_drain_outcome(ShDrainOutcome {
        accepted: vec![3],
        ..ShDrainOutcome::default()
    })
    .unwrap();
    c.promote_composed_clusters();
    assert_eq!(c.state(3), Some(ClusterResidencyState::Sampleable));
    assert_eq!(c.counters().installs, 5, "0, 1, 2, 3 and 3 again");
}

// AC 10, app half (R3): the renderer's half keeps nothing either.
#[test]
fn read_finishing_after_eviction_is_dropped() {
    let (mut c, set) = level();
    frame(&mut c, &set, 0, PORTAL, &[3, 4], 0.0);
    let requests = c.take_requests().unwrap();
    let in_flight = *requests.iter().find(|r| r.cluster_id == 4).unwrap();
    for request in &requests {
        if request.cluster_id != 4 {
            assert_eq!(
                c.admit_prepared(prepared(&c, request.cluster_id)).unwrap(),
                ShDrainAdmission::Ready
            );
        }
    }
    let batch = c.take_async_drain_batch().unwrap();
    let accepted = batch.ready.iter().map(|p| p.chunk.cluster_id).collect();
    c.apply_drain_outcome(ShDrainOutcome {
        accepted,
        ..ShDrainOutcome::default()
    })
    .unwrap();
    c.promote_composed_clusters();

    // Both leave the targets; 3 is evicted while 4's read is still in flight.
    frame(&mut c, &set, 0, PORTAL, &[], 0.1);
    frame(&mut c, &set, 0, PORTAL, &[], 2.2);
    assert_eq!(drain_evictions(&mut c), vec![3]);
    let permits = c.permits_in_use();

    assert_eq!(
        c.admit_prepared(prepared(&c, in_flight.cluster_id))
            .unwrap(),
        ShDrainAdmission::DroppedNotTargeted
    );
    assert_eq!(c.state(4), Some(ClusterResidencyState::Absent));
    assert_eq!(c.permits_in_use(), permits - 1);
    assert_eq!(c.cpu().ready.current_bytes, 0);
    assert_eq!(c.counters().discarded_reads, 1);
    let batch = c.take_async_drain_batch().unwrap();
    assert!(batch.ready.is_empty());
}
