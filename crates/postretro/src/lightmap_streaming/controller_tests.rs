//! Lightmap controller proofs on the test thread (AC 3, 9, 14, 15; P1, P5, P8, P10-P12).
//! See: context/lib/testing_guide.md

use postretro_level_loader::{
    LightmapBlockClass, LightmapDrainOutcome, LightmapPoolReport, LightmapTarget,
};
use postretro_visibility::{VisibilityPath, VisibleCells};

use super::*;
use crate::alloc_probe::AllocSnapshot;
use crate::lightmap_streaming::test_fixtures::*;
use crate::streaming::drain_budget::{DrainClass, DrainItem, DrainRank};
use crate::streaming::request::ReadTier;
use crate::streaming::shared_drain::SharedDrain;

fn mandatory(block: u32, lead_metres: u32) -> Option<LightmapTarget> {
    Some(LightmapTarget {
        block,
        class: LightmapBlockClass::Mandatory,
        lead: lead_metres * M,
    })
}

fn band(block: u32, lead_metres: u32) -> Option<LightmapTarget> {
    Some(LightmapTarget {
        block,
        class: LightmapBlockClass::Band,
        lead: lead_metres * M,
    })
}

fn visible(block: u32) -> Option<LightmapTarget> {
    Some(LightmapTarget {
        block,
        class: LightmapBlockClass::Visible,
        lead: 0,
    })
}

fn targets(rig: &Rig) -> Vec<Option<LightmapTarget>> {
    (0..7).map(|block| rig.controller.target(block)).collect()
}

// ---- AC 3: pins, lead, visibility, priority ----

// AC 3: a cell in a pinned cluster is mandatory from every camera cell,
// including one whose baked range is empty, and ranks as pinned.
#[test]
fn pinned_cluster_block_is_mandatory_from_every_camera_cell() {
    // Cluster 3 holds cells 6 and 7; only cell 6 has a block.
    let hints = corridor_hints(&[3], &[]);
    let mut rig = Rig::corridor(Some(&hints));
    for camera in 0..8 {
        let batch = rig.portal(camera, &[]).unwrap();
        assert_eq!(rig.controller.target(6), mandatory(6, 0), "camera {camera}");
        rig.install_all(&batch, LightmapPoolReport::default());
    }
    let mut first = Rig::corridor(Some(&hints));
    first.portal(0, &[]);
    assert_eq!(
        first.requested_blocks(),
        vec![6, 0, 1, 2],
        "the pin is read before lead-mandatory blocks"
    );
}

// AC 3: an unflagged cell is a target only through lead or visibility.
#[test]
fn unflagged_cell_is_mandatory_only_through_lead_or_visibility() {
    let mut rig = Rig::corridor(None);
    rig.portal(0, &[]);
    assert_eq!(
        rig.controller.target(5),
        None,
        "outside camera 0's baked set"
    );
    assert_eq!(rig.controller.target(3), band(3, 20), "lead 20 m > L");

    rig.controller
        .apply_outcome(LightmapDrainOutcome::default())
        .unwrap();
    rig.portal(0, &[5]);
    assert_eq!(
        rig.controller.target(5),
        visible(5),
        "drawn outside the set"
    );

    rig.controller
        .apply_outcome(LightmapDrainOutcome::default())
        .unwrap();
    rig.portal(4, &[]);
    assert_eq!(
        rig.controller.target(5),
        mandatory(5, 0),
        "within lead of camera 4"
    );

    rig.controller
        .apply_outcome(LightmapDrainOutcome::default())
        .unwrap();
    rig.controller.levers_mut().set_lead_metres(24.0);
    rig.portal(0, &[]);
    assert_eq!(
        rig.controller.target(3),
        mandatory(3, 20),
        "within the raised lead"
    );
    assert_eq!(rig.controller.target(5), None);
}

/// Camera 0's band is blocks 3 (20 m, cluster 1) and 4 (30 m, cluster 2).
/// Returns the band read order once the pool reports `band_blocks` of room
/// and the mandatory pairs are placed, and the order the band pairs drain in.
fn band_order(hints: Option<&ClusterHints>, band_blocks: u64) -> (Vec<u32>, Vec<u32>) {
    let mut rig = Rig::corridor(hints);
    let batch = rig.portal(0, &[]).unwrap();
    assert_eq!(
        rig.requested_blocks(),
        vec![0, 1, 2],
        "band waits for headroom"
    );
    rig.install_all(&batch, headroom(band_blocks));
    rig.complete_all();
    let batch = rig.portal(0, &[]).unwrap();
    assert!(
        rig.requests.is_empty(),
        "band waits for the mandatory pairs in hand"
    );
    rig.install_all(&batch, headroom(band_blocks));
    let batch = rig.portal(0, &[]).unwrap();
    let band_reads = rig.requested_blocks();
    rig.install_all(&batch, headroom(band_blocks));
    rig.complete_all();
    let batch = rig.portal(0, &[]).unwrap();
    let drained = batch.ready.iter().map(|prepared| prepared.block).collect();
    (band_reads, drained)
}

// AC 3: across the same band blocks, a priority region reorders prefetch
// reads and drain admission.
#[test]
fn priority_region_reorders_band_prefetch() {
    assert_eq!(band_order(None, 2), (vec![3, 4], vec![3, 4]), "lead order");
    let hints = corridor_hints(&[], &[(2, 3)]);
    assert_eq!(
        band_order(Some(&hints), 2),
        (vec![4, 3], vec![4, 3]),
        "priority 3 on cluster 2 outranks the nearer block"
    );
    assert_eq!(
        band_order(Some(&hints), 1).0,
        vec![4],
        "one block of room goes to the priority region"
    );
}

// ---- AC 9 (CPU side): band reads against headroom, refusals ----

// AC 9: mandatory reads ignore headroom; band reads stop at the headroom
// left once the mandatory pairs in hand are placed.
#[test]
fn band_reads_stop_at_the_pool_headroom_while_mandatory_reads_do_not() {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(0, &[]).unwrap();
    assert_eq!(rig.requested_blocks(), vec![0, 1, 2], "no headroom yet");
    rig.install_all(&batch, headroom(1));
    rig.complete_all();
    let batch = rig.portal(0, &[]).unwrap();
    assert!(
        rig.requests.is_empty(),
        "the three mandatory pairs in hand claim the one block of room first"
    );
    rig.install_all(&batch, headroom(1));
    rig.portal(0, &[]);
    assert_eq!(rig.requested_blocks(), vec![3], "one block of band room");
}

// AC 9: a refused band block is not requested again until the pool reports
// room for one more of its slots than it had when it refused.
#[test]
fn refused_band_block_is_not_requested_again_until_headroom_grows() {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(1));
    rig.complete_all();
    // The mandatory pairs install, then block 3 is read into the band room.
    for _ in 0..2 {
        let batch = rig.portal(0, &[]).unwrap();
        rig.install_all(&batch, headroom(1));
        rig.complete_all();
    }
    let batch = rig.portal(0, &[]).unwrap();
    assert_eq!(
        batch.ready.iter().map(|p| p.block).collect::<Vec<_>>(),
        vec![3]
    );
    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            refused: vec![3],
            pool: headroom(1),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    assert_eq!(rig.controller.phase(3), BlockPhase::Refused);

    for _ in 0..4 {
        let batch = rig.portal(0, &[]).unwrap();
        rig.install_all(&batch, headroom(1));
        rig.complete_all();
    }
    assert_eq!(
        rig.controller.counters().reads_requested,
        5,
        "3 once, then 4"
    );
    assert_eq!(rig.controller.phase(3), BlockPhase::Refused);

    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(4));
    rig.portal(0, &[]);
    assert_eq!(
        rig.requested_blocks(),
        vec![3],
        "room grew past the refusal"
    );
}

// ---- AC 14: one test per visibility path ----

#[test]
fn portal_walk_path_demands_drawn_cells_outside_the_baked_set_as_visible() {
    let mut rig = Rig::corridor(None);
    rig.portal(0, &[0, 5]);
    assert_eq!(rig.controller.target(5), visible(5));
    assert_eq!(rig.requested_blocks(), vec![5, 0, 1, 2]);
    assert_eq!(rig.requests[0].tier, ReadTier::Mandatory);
}

fn camera_set_only(path: VisibilityPath) {
    let mut rig = Rig::corridor(None);
    rig.frame(0, path, &VisibleCells::Culled(vec![0, 5, 6]));
    assert_eq!(rig.controller.target(5), None, "frustum set adds nothing");
    assert_eq!(rig.controller.target(6), None);
    assert_eq!(rig.requested_blocks(), vec![0, 1, 2]);
    assert!(
        rig.requests
            .iter()
            .all(|request| [0, 1, 2].contains(&request.key))
    );
}

#[test]
fn portal_step_limit_path_demands_only_the_camera_cells_baked_set() {
    camera_set_only(VisibilityPath::PortalStepLimitFallback {
        considered: 10,
        accepted: 3,
    });
}

#[test]
fn no_portals_path_demands_only_the_camera_cells_baked_set() {
    camera_set_only(VisibilityPath::NoPortalsFallback);
}

/// A solid or exterior camera cell with no baked entries keeps current
/// demand and requests nothing new, even with requestable work pending.
fn empty_camera_cell_holds(path: VisibilityPath) {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(0, &[0, 5]).unwrap();
    rig.requests.clear();
    // Headroom arrives: camera 0's band would be requested next frame.
    rig.install_all(&batch, headroom(8));
    let before = targets(&rig);
    let lookups = rig.controller.counters().residency_lookups;

    let batch = rig
        .frame(7, path, &VisibleCells::Culled(vec![1, 2, 3]))
        .unwrap();
    assert_eq!(targets(&rig), before, "residency held, visible demand kept");
    assert!(rig.requests.is_empty(), "requests nothing new");
    assert_eq!(rig.controller.counters().residency_lookups, lookups + 1);

    rig.install_all(&batch, headroom(8));
    let batch = rig.portal(0, &[0, 5]).unwrap();
    assert_eq!(rig.requested_blocks(), vec![3, 4], "held work resumes");

    // A solid or exterior cell that has a baked set demands it.
    rig.install_all(&batch, headroom(8));
    rig.frame(6, path, &VisibleCells::Culled(vec![1]));
    assert_eq!(rig.controller.target(6), mandatory(6, 0));
    assert_eq!(rig.controller.target(0), None);
}

#[test]
fn solid_cell_path_with_an_empty_camera_set_holds_residency_and_requests_nothing() {
    empty_camera_cell_holds(VisibilityPath::SolidCellFallback);
}

#[test]
fn exterior_cell_path_with_an_empty_camera_set_holds_residency_and_requests_nothing() {
    empty_camera_cell_holds(VisibilityPath::ExteriorCellFallback);
}

#[test]
fn empty_world_path_performs_no_residency_lookup_and_requests_nothing() {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(0, &[]).unwrap();
    rig.requests.clear();
    rig.install_all(&batch, headroom(8));
    let before = targets(&rig);
    let lookups = rig.controller.counters().residency_lookups;

    rig.frame(
        3,
        VisibilityPath::EmptyWorldFallback,
        &VisibleCells::DrawAll,
    );
    assert_eq!(rig.controller.counters().residency_lookups, lookups);
    assert_eq!(targets(&rig), before);
    assert!(rig.requests.is_empty());
}

// ---- AC 15: steady frames and camera-cell changes ----

// AC 15: steady frames with an unchanged camera cell emit no target deltas
// and allocate nothing in lightmap residency: every per-frame buffer keeps
// its capacity and the counting allocator sees no allocation.
#[test]
fn steady_frames_emit_no_deltas_and_allocate_nothing() {
    let mut rig = Rig::corridor(None);
    rig.settle(2, &[2, 3, 6], headroom(8));
    for block in [0, 1, 2, 3, 4, 5, 6] {
        assert_eq!(
            rig.controller.phase(block),
            BlockPhase::Installed,
            "{block}"
        );
    }
    let visible = VisibleCells::Culled(vec![2, 3, 6]);
    let capacities = rig.controller.buffer_capacities();
    let drain_capacity = rig.drain.capacity();
    let counters = rig.controller.counters();

    let probe = AllocSnapshot::arm();
    for _ in 0..64 {
        let batch = rig.frame(2, PORTAL, &visible).unwrap();
        assert!(batch.target_set.is_empty() && batch.target_remove.is_empty());
        assert!(batch.ready.is_empty());
        rig.install_all(&batch, headroom(8));
    }
    assert_eq!(
        probe.allocs_since(),
        0,
        "no heap allocation in steady frames"
    );

    assert_eq!(rig.controller.buffer_capacities(), capacities);
    assert_eq!(rig.drain.capacity(), drain_capacity);
    let after = rig.controller.counters();
    assert_eq!(after.target_deltas, counters.target_deltas, "zero deltas");
    assert_eq!(after.baked_recomputes, counters.baked_recomputes);
    assert_eq!(after.residency_lookups, counters.residency_lookups);
    assert_eq!(after.reads_requested, counters.reads_requested);
    assert!(rig.requests.is_empty());
}

// AC 15: a camera-cell change emits only the entries whose target changed.
#[test]
fn camera_cell_change_emits_only_the_changed_entries() {
    let mut rig = Rig::corridor(None);
    rig.settle(2, &[], headroom(8));
    let deltas = rig.controller.counters().target_deltas;

    let batch = rig.portal(3, &[]).unwrap();
    // Camera 2: 0 M12, 1 M0, 2 M0, 3 M0, 4 M12, 5 B28.
    // Camera 3: 1 M14, 2 M0, 3 M0, 4 M0, 5 M14.
    assert_eq!(
        batch.target_set,
        vec![
            mandatory(1, 14).unwrap(),
            mandatory(4, 0).unwrap(),
            mandatory(5, 14).unwrap(),
        ]
    );
    assert_eq!(batch.target_remove, vec![0]);
    assert!(batch.target_reset.is_none());
    assert_eq!(rig.controller.counters().target_deltas, deltas + 4);
}

// ---- Pin rows ----

// P1: a camera-cell change and a lead change in the same frame recompute
// demand once, to the new cell's entries split at the new lead.
#[test]
fn camera_cell_and_lead_change_in_one_frame_recompute_demand_once() {
    let mut rig = Rig::corridor(None);
    rig.settle(2, &[], headroom(8));
    let recomputes = rig.controller.counters().baked_recomputes;

    rig.controller.levers_mut().set_lead_metres(10.0);
    rig.portal(3, &[]);
    assert_eq!(rig.controller.counters().baked_recomputes, recomputes + 1);
    assert_eq!(
        targets(&rig),
        vec![
            None,
            band(1, 14),
            mandatory(2, 0),
            mandatory(3, 0),
            mandatory(4, 0),
            band(5, 14),
            None,
        ]
    );
}

// P8 (controller side): a lowered cap rides the next batch; the renderer
// may evict band blocks for it, never mandatory or visible ones.
#[test]
fn lowered_pool_cap_rides_the_next_batch_and_only_band_blocks_may_be_evicted() {
    let mut rig = Rig::corridor(None);
    rig.settle(0, &[5], headroom(8));
    assert_eq!(
        rig.controller.phase(4),
        BlockPhase::Installed,
        "band resident"
    );

    rig.controller.levers_mut().set_pool_cap_layers(1);
    let batch = rig.portal(0, &[5]).unwrap();
    assert_eq!(batch.pool_cap_layers, 1);
    for protected in [0, 5] {
        let error = rig
            .controller
            .apply_outcome(LightmapDrainOutcome {
                evicted: vec![protected],
                ..LightmapDrainOutcome::default()
            })
            .unwrap_err();
        assert!(
            error.to_string().contains("cannot be evicted"),
            "block {protected}: {error}"
        );
    }
    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            evicted: vec![4],
            pool: headroom(0),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    assert_eq!(rig.controller.phase(4), BlockPhase::Absent);
    assert_eq!(rig.controller.target(4), band(4, 30), "still a band target");
    rig.portal(0, &[5]);
    assert!(rig.requests.is_empty(), "no room to read it back");
}

// P10: a spawn cell with an empty baked range installs nothing and requests
// nothing; its first batch is an empty reset.
#[test]
fn spawn_cell_with_an_empty_baked_range_starts_empty_and_requests_nothing() {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(7, &[7]).unwrap();
    assert_eq!(batch.target_reset, Some(Vec::new()));
    assert!(batch.ready.is_empty());
    assert!(rig.requests.is_empty());
    rig.install_all(&batch, headroom(8));
    let batch = rig.portal(7, &[7]).unwrap();
    assert!(batch.target_set.is_empty() && batch.ready.is_empty());
    assert!(rig.requests.is_empty());
}

// P11: on the first portal-walk frame, a drawn cell outside the baked set is
// demanded visible at the mandatory tier, counts as a miss both outside the
// baked set and not resident, and may never be refused.
#[test]
fn drawn_cell_outside_the_baked_set_is_visible_demand_never_refused() {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(0, &[0, 5]).unwrap();
    assert_eq!(rig.controller.target(5), visible(5));
    let request = rig
        .requests
        .iter()
        .find(|request| request.key == 5)
        .unwrap();
    assert_eq!(request.tier, ReadTier::Mandatory);
    rig.controller.count_visible_misses();
    let counters = rig.controller.counters();
    assert_eq!(counters.last_frame_drawn_outside_baked_set, 1, "block 5");
    assert_eq!(counters.last_frame_drawn_not_resident, 2, "blocks 0 and 5");
    assert!(
        batch
            .target_reset
            .as_ref()
            .unwrap()
            .contains(&visible(5).unwrap())
    );

    rig.install_all(&batch, headroom(8));
    rig.complete_all();
    let batch = rig.portal(0, &[0, 5]).unwrap();
    assert!(batch.ready.iter().any(|prepared| prepared.block == 5));
    let error = rig
        .controller
        .apply_outcome(LightmapDrainOutcome {
            refused: vec![5],
            installed: batch
                .ready
                .iter()
                .map(|prepared| prepared.block)
                .filter(|&block| block != 5)
                .collect(),
            ..LightmapDrainOutcome::default()
        })
        .unwrap_err();
    assert!(error.to_string().contains("not a band target"), "{error}");
}

// P12: SH and lightmap mandatory demand over one budget, across consecutive
// drains: each drain admits at least one item, every pair drains whole, and
// both resources finish.
#[test]
fn shared_drain_admits_sh_and_lightmap_mandatory_work_without_starving_either() {
    const HALF: u64 = 3 * MIB / 2;
    let mut rig = Rig::new(corridor_blocks(HALF, true), corridor_set(), None);
    let batch = rig.portal(2, &[6]).unwrap();
    rig.install_all(&batch, LightmapPoolReport::default());
    rig.complete_all();

    // SH ready clusters, 3 MiB each: two visible, two pinned.
    let mut sh: Vec<DrainItem> = [
        (DrainClass::Visible, 10),
        (DrainClass::Visible, 11),
        (DrainClass::Pinned, 12),
        (DrainClass::Pinned, 13),
    ]
    .into_iter()
    .map(|(class, cluster)| DrainItem {
        rank: DrainRank::sh(class, 0, cluster),
        bytes: 3 * MIB,
    })
    .collect();

    let mut drain = SharedDrain::default();
    let mut order = Vec::new();
    let mut lightmap_drained = Vec::new();
    for _ in 0..8 {
        drain.begin();
        for &item in &sh {
            drain.offer(item);
        }
        rig.controller.offer_ready(&mut drain).unwrap();
        drain.admit().unwrap();
        let sh_admitted: Vec<u32> = drain.admitted_keys(StreamResource::Sh).collect();
        sh.retain(|item| !sh_admitted.contains(&item.rank.key()));
        let batch = rig.controller.finish_drain(&drain).unwrap().unwrap();
        for prepared in &batch.ready {
            let payload = &prepared.payload;
            assert_eq!(
                (payload.irradiance.len() + payload.direction.len()) as u64,
                HALF
            );
            let [group_a, group_b] = payload.shadowmask.as_ref().expect("pair whole");
            assert_eq!((group_a.len() + group_b.len()) as u64, HALF);
            lightmap_drained.push(prepared.block);
        }
        let admitted = sh_admitted.len() + batch.ready.len();
        if admitted == 0 {
            rig.install_all(&batch, LightmapPoolReport::default());
            break;
        }
        order.push((
            sh_admitted,
            batch.ready.iter().map(|p| p.block).collect::<Vec<_>>(),
        ));
        rig.install_all(&batch, LightmapPoolReport::default());
    }
    assert_eq!(
        order,
        vec![
            (vec![10, 11], vec![]),
            (vec![12], vec![6]),
            (vec![13], vec![1]),
            (vec![], vec![2, 3]),
            (vec![], vec![0, 4]),
        ],
        "visible across both, then pinned, then lead (nearest first)"
    );
    assert!(sh.is_empty());
    lightmap_drained.sort_unstable();
    assert_eq!(lightmap_drained, vec![0, 1, 2, 3, 4, 6], "each pair once");
}

// P5 (controller half): demand for a block already read, ready, or
// installed never submits it again.
#[test]
fn repeated_demand_after_delivery_never_requests_a_block_again() {
    let mut rig = Rig::corridor(None);
    rig.portal(6, &[]);
    assert_eq!(
        rig.requested_blocks(),
        vec![5, 6],
        "equal lead: block order"
    );
    // Delivered but not yet drained: the previous batch is outstanding.
    rig.complete_all();
    for _ in 0..3 {
        assert!(rig.portal(6, &[]).is_none(), "outcome outstanding");
    }
    assert!(rig.requests.is_empty());
    rig.controller
        .apply_outcome(LightmapDrainOutcome::default())
        .unwrap();
    let batch = rig.portal(6, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    for _ in 0..3 {
        let batch = rig.portal(6, &[]).unwrap();
        rig.install_all(&batch, headroom(8));
    }
    assert!(rig.requests.is_empty());
    assert_eq!(rig.controller.counters().reads_requested, 2);
    assert_eq!(rig.controller.counters().installs, 2);
}

// AC 13 (controller side): a completion from another generation is dropped
// with its buffers and changes nothing.
#[test]
fn completion_from_a_previous_generation_is_discarded() {
    let mut old = Rig::corridor(None);
    old.portal(6, &[]);
    let stale = old.source.completion(old.requests[0]);

    let mut rig = Rig::corridor(None);
    rig.portal(6, &[]);
    assert_ne!(old.controller.generation(), rig.controller.generation());
    rig.controller.admit_completion(stale).unwrap();
    assert_eq!(rig.controller.counters().stale_completions, 1);
    assert_eq!(rig.controller.phase(6), BlockPhase::InFlight);
    assert_eq!(rig.controller.permits_in_use(), 2);
}

// A pair the renderer could not install lands in `failed`, whatever its
// class, and is handled as a failed read: counted, warned once, not re-read
// while demanded, retried once after it leaves demand and returns.
#[test]
fn failed_mandatory_install_is_not_reread_and_retries_once_after_leaving_demand() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(6, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    rig.complete_all();
    let batch = rig.portal(6, &[]).unwrap();
    assert_eq!(batch.ready.len(), 2);
    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            installed: vec![5],
            failed: vec![6],
            pool: headroom(8),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    assert_eq!(rig.controller.target(6), mandatory(6, 0), "still demanded");
    assert_eq!(rig.controller.phase(6), BlockPhase::Failed);
    assert_eq!(rig.controller.counters().failed_installs, 1);
    assert_eq!(rig.controller.in_hand_bytes(), 0, "its payload is released");
    for _ in 0..4 {
        let batch = rig.portal(6, &[]).unwrap();
        rig.install_all(&batch, headroom(8));
    }
    assert!(rig.requests.is_empty(), "no re-read while demanded");

    // Leaving demand and returning allows one retry.
    let leave_and_return = |rig: &mut Rig| {
        let batch = rig.portal(0, &[]).unwrap();
        rig.install_all(&batch, headroom(8));
        rig.requests.clear();
        let batch = rig.portal(6, &[]).unwrap();
        rig.install_all(&batch, headroom(8));
    };
    leave_and_return(&mut rig);
    assert_eq!(rig.requested_blocks(), vec![6], "the one retry");
    rig.complete_all();
    let batch = rig.portal(6, &[]).unwrap();
    assert_eq!(batch.ready.len(), 1);
    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            failed: vec![6],
            pool: headroom(8),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    assert_eq!(rig.controller.counters().failed_installs, 2);

    leave_and_return(&mut rig);
    assert!(rig.requests.is_empty(), "the retry is spent");
    assert_eq!(rig.controller.phase(6), BlockPhase::Failed);
    capture.assert_logged_once(log::Level::Warn, "block 6 install failed");
}

// An outcome must account for a failed pair exactly once, like any other.
#[test]
fn outcome_naming_a_pair_both_installed_and_failed_is_rejected() {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(6, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    rig.complete_all();
    rig.portal(6, &[]).unwrap();
    let error = rig
        .controller
        .apply_outcome(LightmapDrainOutcome {
            installed: vec![5, 6],
            failed: vec![6],
            ..LightmapDrainOutcome::default()
        })
        .unwrap_err();
    assert!(
        error.to_string().contains("every drained pair once"),
        "{error}"
    );
    assert_eq!(
        rig.controller.phase(6),
        BlockPhase::InDrain,
        "nothing applied"
    );
}

// ---- Non-portal frames hold what they draw ----

const STEP_LIMIT: VisibilityPath = VisibilityPath::PortalStepLimitFallback {
    considered: 10,
    accepted: 3,
};

// A non-portal frame keeps the target of a drawn block that is resident, so
// the renderer never frees a block the frame draws, and reads nothing for
// it. The hold ends once the block is no longer drawn.
#[test]
fn step_limit_frame_keeps_a_drawn_resident_visible_block_without_reading() {
    let mut rig = Rig::corridor(None);
    rig.settle(0, &[5], headroom(8));
    assert_eq!(rig.controller.phase(5), BlockPhase::Installed);
    assert_eq!(rig.controller.target(5), visible(5));
    let reads = rig.controller.counters().reads_requested;

    let batch = rig
        .frame(0, STEP_LIMIT, &VisibleCells::Culled(vec![0, 5]))
        .unwrap();
    assert_eq!(
        rig.controller.target(5),
        visible(5),
        "held while drawn and resident"
    );
    assert!(batch.target_remove.is_empty(), "the renderer keeps block 5");
    assert!(rig.requests.is_empty());
    assert_eq!(rig.controller.counters().reads_requested, reads);
    rig.install_all(&batch, headroom(8));

    let batch = rig
        .frame(0, STEP_LIMIT, &VisibleCells::Culled(vec![0]))
        .unwrap();
    assert_eq!(rig.controller.target(5), None, "no longer drawn");
    assert_eq!(batch.target_remove, vec![5]);
}

// A drawn block whose read is already in flight is held too: the read is
// not cancelled under the frame that draws it.
#[test]
fn step_limit_frame_keeps_a_drawn_block_whose_read_is_in_flight() {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(0, &[0, 5]).unwrap();
    assert!(rig.requested_blocks().contains(&5));
    rig.install_all(&batch, headroom(8));

    rig.frame(0, STEP_LIMIT, &VisibleCells::Culled(vec![5]));
    assert_eq!(rig.controller.target(5), visible(5));
    assert_eq!(rig.controller.phase(5), BlockPhase::InFlight);
}

// A drawn block that is not resident is neither held nor requested on a
// non-portal frame (it lies outside the camera cell's baked set), and it
// counts as a visible miss in both buckets.
#[test]
fn step_limit_frame_neither_reads_nor_holds_a_drawn_absent_block_and_counts_it() {
    let mut rig = Rig::corridor(None);
    rig.settle(0, &[], headroom(8));

    let batch = rig
        .frame(0, STEP_LIMIT, &VisibleCells::Culled(vec![0, 5]))
        .unwrap();
    assert_eq!(rig.controller.target(5), None);
    assert!(
        rig.requests.is_empty(),
        "outside the camera cell's baked set"
    );
    rig.install_all(&batch, headroom(8));
    rig.controller.count_visible_misses();
    let counters = rig.controller.counters();
    assert_eq!(counters.last_frame_drawn_not_resident, 1, "block 5");
    assert_eq!(counters.last_frame_drawn_outside_baked_set, 1, "block 5");
}

// ---- Band reads leave mandatory work its reserve ----

// Band reads may hold at most half the permits; the other half is a reserve
// mandatory and visible reads always get, even with the band in flight.
#[test]
fn band_reads_hold_at_most_half_the_permits_and_mandatory_reads_use_the_reserve() {
    // Camera 0: cell 0 at lead 0 and 24 band cells. Camera 1: 15 new cells
    // at lead 0.
    let mut rows = vec![(0, 0, 0)];
    rows.extend((1..=24).map(|cell| (0, cell, 20)));
    rows.extend((25..=39).map(|cell| (1, cell, 0)));
    let blocks = (0..40)
        .map(|block| BlockSpec::standard(block, block, 64, true))
        .collect();
    let mut rig = Rig::new(blocks, residency_set(40, &rows, 32), None);
    let room = LightmapPoolReport {
        layers: 1,
        band_headroom_texels: 1 << 30,
        ..LightmapPoolReport::default()
    };

    let batch = rig.portal(0, &[]).unwrap();
    assert_eq!(rig.requested_blocks(), vec![0]);
    rig.install_all(&batch, room);
    rig.complete_all();
    let batch = rig.portal(0, &[]).unwrap();
    assert_eq!(
        rig.requested_blocks(),
        (1..=16).collect::<Vec<_>>(),
        "16 of the 24 band blocks: half the permits"
    );
    rig.install_all(&batch, room);
    rig.requests.clear();

    // The band reads are still in flight when camera 1's set arrives.
    rig.portal(1, &[]);
    assert_eq!(
        rig.requested_blocks(),
        (25..=39).collect::<Vec<_>>(),
        "every mandatory read fits the reserve"
    );
    assert_eq!(rig.controller.permits_in_use(), 16 + 15);
}

// A band read promoted in flight is resubmitted at the mandatory tier, so
// the issuer raises the pending request's tier; the permit is not taken
// twice, and the pair installs once.
#[test]
fn band_read_promoted_in_flight_is_resubmitted_at_the_mandatory_tier() {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    rig.complete_all();
    let batch = rig.portal(0, &[]).unwrap();
    let band: Vec<(u32, ReadTier)> = rig
        .requests
        .iter()
        .map(|request| (request.key, request.tier))
        .collect();
    assert_eq!(band, vec![(3, ReadTier::Optional), (4, ReadTier::Optional)]);
    rig.install_all(&batch, headroom(8));
    rig.requests.clear();

    // Camera 2 makes blocks 3 (lead 0) and 4 (12 m) mandatory while their
    // reads are in flight; block 5 is its band.
    let batch = rig.portal(2, &[]).unwrap();
    let raised: Vec<u32> = rig
        .requests
        .iter()
        .filter(|request| request.tier == ReadTier::Mandatory)
        .map(|request| request.key)
        .collect();
    assert_eq!(raised, vec![3, 4]);
    assert_eq!(rig.controller.counters().tier_raises, 2);
    assert_eq!(rig.controller.phase(3), BlockPhase::InFlight);
    assert_eq!(rig.controller.permits_in_use(), 3, "blocks 3, 4 and 5");
    rig.install_all(&batch, headroom(8));

    let installs = rig.controller.counters().installs;
    rig.complete_all();
    let batch = rig.portal(2, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    assert_eq!(rig.controller.phase(3), BlockPhase::Installed);
    assert_eq!(rig.controller.counters().installs, installs + 3);
    assert_eq!(rig.controller.counters().duplicate_completions, 0);
}

// ---- Band headroom in pool slots ----

// Band reads are charged the block's pool slot, its extent rounded up to the
// slot alignment, not its raw texels.
#[test]
fn band_reads_charge_the_aligned_slot_of_each_block() {
    let mut blocks = corridor_blocks(64, true);
    for block in [3, 4] {
        // 62 × 62 texels take a 64 × 64 slot.
        blocks[block].width = 62;
        blocks[block].height = 62;
    }
    let mut rig = Rig::new(blocks, corridor_set(), None);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(0));
    rig.complete_all();
    let batch = rig.portal(0, &[]).unwrap();
    // Room for both blocks' raw texels (2 × 3844), not for both slots.
    let room = LightmapPoolReport {
        layers: 1,
        band_headroom_texels: 7_700,
        ..LightmapPoolReport::default()
    };
    rig.install_all(&batch, room);
    rig.portal(0, &[]);
    assert_eq!(rig.requested_blocks(), vec![3], "one 64 × 64 slot of room");
}

// A refused band block is re-read only once the pool has room for a whole
// slot more than at its refusal, not on any small gain.
#[test]
fn refused_band_block_waits_for_a_whole_slot_of_new_headroom() {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(1));
    rig.complete_all();
    for _ in 0..2 {
        let batch = rig.portal(0, &[]).unwrap();
        rig.install_all(&batch, headroom(1));
        rig.complete_all();
    }
    rig.portal(0, &[]).unwrap();
    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            refused: vec![3],
            pool: headroom(1),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    let room = |texels| LightmapPoolReport {
        layers: 1,
        band_headroom_texels: texels,
        ..LightmapPoolReport::default()
    };

    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, room(2 * 64 * 64 - 1));
    assert_eq!(
        rig.controller.phase(3),
        BlockPhase::Refused,
        "short of a slot"
    );
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, room(2 * 64 * 64));
    assert_eq!(
        rig.controller.phase(3),
        BlockPhase::Absent,
        "a whole slot more"
    );
}

// ---- Renderer failures ----

// A drain the renderer rolled back returns every drained pair to Absent,
// releases its permit, and re-sends every target in the next batch.
#[test]
fn aborted_drain_returns_its_pairs_and_resets_the_targets() {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(6, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    rig.complete_all();
    let batch = rig.portal(6, &[]).unwrap();
    assert_eq!(batch.ready.len(), 2);

    rig.controller.abort_drain();
    for block in [5, 6] {
        assert_eq!(rig.controller.phase(block), BlockPhase::Absent);
    }
    assert_eq!(rig.controller.permits_in_use(), 0);
    assert_eq!(rig.controller.in_hand_bytes(), 0);
    let batch = rig.portal(6, &[]).expect("no outcome is outstanding");
    assert!(batch.target_reset.is_some());
    assert_eq!(rig.requested_blocks(), vec![5, 6], "read again");
}
