use super::*;
use crate::sh_streaming::generation::FixedGenerationClock;
use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_visibility::VisibleCells;

fn topology(
    cell_to_cluster: Vec<u32>,
    adjacency: Vec<Vec<u32>>,
    owners: Vec<Vec<u32>>,
    requested_resident_bytes: Vec<u64>,
) -> PlannerTopology {
    let cluster_count = adjacency.len();
    assert_eq!(owners.len(), cluster_count);
    assert_eq!(requested_resident_bytes.len(), cluster_count);
    PlannerTopology {
        cell_to_cluster,
        adjacency,
        owners,
        requested_resident_bytes,
        chunk_hashes: (0..cluster_count)
            .map(|cluster_id| [cluster_id as u8; 32])
            .collect(),
    }
}

fn controller(topology: PlannerTopology) -> ShResidencyController {
    ShResidencyController::for_test(topology, &FixedGenerationClock::new(1)).unwrap()
}

fn prepared(controller: &ShResidencyController, cluster_id: u32) -> PreparedShCluster {
    PreparedShCluster {
        generation: controller.generation(),
        content_tag: controller.content_tag(),
        chunk: DecodedClusterShPayload {
            cluster_id,
            bytes: vec![cluster_id as u8; 4],
            blocks: Vec::new(),
        },
    }
}

fn queue_ready(controller: &mut ShResidencyController, expected_cluster: u32) {
    let request = controller.take_next_request().unwrap().unwrap();
    assert_eq!(request.cluster_id, expected_cluster);
    assert_eq!(
        controller
            .admit_prepared(prepared(controller, expected_cluster))
            .unwrap(),
        ShDrainAdmission::Ready
    );
}

fn accept_ready(controller: &mut ShResidencyController) {
    let batch = controller.take_drain_batch().unwrap();
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
}

#[test]
fn visible_cells_drive_two_hop_targets_and_time_based_hysteresis() {
    let mut controller = controller(topology(
        vec![0, 1, 2, 3],
        vec![vec![1], vec![0, 2], vec![1, 3], vec![2]],
        vec![vec![], vec![], vec![], vec![]],
        vec![1; 4],
    ));

    controller
        .update_targets(&VisibleCells::Culled(vec![0]), 0.0)
        .unwrap();
    assert!(controller.is_targeted(0));
    assert!(controller.is_targeted(1));
    assert!(controller.is_targeted(2));
    assert!(!controller.is_targeted(3));

    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), 0.5)
        .unwrap();
    assert!(controller.is_targeted(0));
    assert!(controller.is_targeted(1));
    assert!(controller.is_targeted(2));

    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), 2.49)
        .unwrap();
    assert!(controller.is_targeted(0));
    assert!(controller.is_targeted(1));
    assert!(controller.is_targeted(2));

    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), 2.5)
        .unwrap();
    assert!(!controller.is_targeted(0));
    assert!(!controller.is_targeted(1));
    assert!(!controller.is_targeted(2));
}

#[test]
fn owner_installs_and_promotes_before_dependent_halo_is_drained() {
    let mut controller = controller(topology(
        vec![0, 1],
        vec![vec![], vec![]],
        vec![vec![], vec![0]],
        vec![8, 16],
    ));
    controller
        .update_targets(&VisibleCells::Culled(vec![1]), 0.0)
        .unwrap();
    assert!(
        controller.is_targeted(0),
        "owner closure must target the owner"
    );

    queue_ready(&mut controller, 0);
    let owner_batch = controller.take_drain_batch().unwrap();
    assert_eq!(
        owner_batch
            .ready
            .iter()
            .map(|prepared| prepared.chunk.cluster_id)
            .collect::<Vec<_>>(),
        vec![0]
    );
    controller
        .apply_drain_outcome(ShDrainOutcome {
            accepted: vec![0],
            ..ShDrainOutcome::default()
        })
        .unwrap();
    assert_eq!(
        controller.state(0),
        Some(ClusterResidencyState::InstalledUncomposed)
    );
    controller.promote_composed_clusters();

    queue_ready(&mut controller, 1);
    let halo_batch = controller.take_drain_batch().unwrap();
    assert_eq!(
        halo_batch
            .ready
            .iter()
            .map(|prepared| prepared.chunk.cluster_id)
            .collect::<Vec<_>>(),
        vec![1]
    );
}

#[test]
fn missing_owner_defers_a_ready_halo_without_transferring_ownership() {
    let mut controller = controller(topology(
        vec![0, 1],
        vec![vec![], vec![]],
        vec![vec![], vec![0]],
        vec![1, 1],
    ));
    controller
        .update_targets(&VisibleCells::Culled(vec![1]), 0.0)
        .unwrap();

    controller.states[1].state = ClusterResidencyState::Ready;
    controller.ready.insert(
        1,
        ReadyCluster {
            prepared: prepared(&controller, 1),
            byte_charge: 4,
        },
    );
    controller.permits_in_use = 1;
    controller
        .accounting
        .cpu
        .ready
        .add(4, "ready bytes")
        .unwrap();

    let batch = controller.take_drain_batch().unwrap();
    assert!(batch.ready.is_empty());
    assert_eq!(controller.state(1), Some(ClusterResidencyState::Ready));
    assert!(controller.ready.contains_key(&1));
}

#[test]
fn zero_one_and_many_cluster_lifecycles_keep_reset_and_install_caps_bounded() {
    let mut empty = controller(topology(Vec::new(), Vec::new(), Vec::new(), Vec::new()));
    empty.update_targets(&VisibleCells::DrawAll, 0.0).unwrap();
    assert_eq!(
        empty.take_drain_batch().unwrap().target_reset,
        Some(Vec::new())
    );

    let mut one = controller(topology(vec![0], vec![vec![]], vec![vec![]], vec![32]));
    one.update_targets(&VisibleCells::Culled(vec![0]), 0.0)
        .unwrap();
    queue_ready(&mut one, 0);
    accept_ready(&mut one);
    assert_eq!(one.permits_in_use(), 0);
    assert_eq!(one.accounting().logical_occupancy_bytes, 32);
    one.promote_composed_clusters();
    assert_eq!(one.state(0), Some(ClusterResidencyState::Sampleable));

    let mut many = controller(topology(
        vec![0, 1, 2],
        vec![vec![], vec![], vec![]],
        vec![vec![], vec![], vec![]],
        vec![1, 1, 1],
    ));
    many.update_targets(&VisibleCells::DrawAll, 0.0).unwrap();
    let reset = many.take_drain_batch().unwrap();
    assert_eq!(reset.target_reset, Some(vec![0b111]));
    queue_ready(&mut many, 0);
    queue_ready(&mut many, 1);
    queue_ready(&mut many, 2);
    let batch = many.take_drain_batch().unwrap();
    assert_eq!(batch.ready.len(), MAX_INSTALLS_PER_DRAIN);
    assert_eq!(many.permits_in_use(), 3);
}

#[test]
fn old_generation_completion_cannot_consume_a_new_request_permit() {
    let mut controller = controller(topology(vec![0], vec![vec![]], vec![vec![]], vec![1]));
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), 0.0)
        .unwrap();
    let request = controller.take_next_request().unwrap().unwrap();
    let mut stale = prepared(&controller, request.cluster_id);
    stale.generation += 1;
    assert_eq!(
        controller.admit_prepared(stale).unwrap(),
        ShDrainAdmission::DroppedStale
    );
    assert_eq!(controller.permits_in_use(), 1);
    assert_eq!(controller.state(0), Some(ClusterResidencyState::Queued));
    assert_eq!(
        controller
            .admit_prepared(prepared(&controller, request.cluster_id))
            .unwrap(),
        ShDrainAdmission::Ready
    );
}

#[test]
fn four_lifecycle_permits_bound_queued_work() {
    let mut controller = controller(topology(
        vec![0, 1, 2, 3, 4],
        vec![vec![], vec![], vec![], vec![], vec![]],
        vec![vec![], vec![], vec![], vec![], vec![]],
        vec![1; 5],
    ));
    controller
        .update_targets(&VisibleCells::DrawAll, 0.0)
        .unwrap();

    for expected_cluster in 0..MAX_STREAM_PERMITS as u32 {
        assert_eq!(
            controller.take_next_request().unwrap().unwrap().cluster_id,
            expected_cluster
        );
    }
    assert!(controller.take_next_request().unwrap().is_none());
    assert_eq!(controller.permits_in_use(), MAX_STREAM_PERMITS);
}

#[test]
fn visible_dependency_owner_beats_a_hysteresis_ready_backlog() {
    let mut controller = controller(topology(
        vec![0, 1, 2, 3],
        vec![vec![], vec![], vec![], vec![]],
        vec![vec![], vec![0], vec![], vec![]],
        vec![1; 4],
    ));
    controller
        .update_targets(&VisibleCells::DrawAll, 0.0)
        .unwrap();
    controller.take_drain_batch().unwrap();
    controller
        .update_targets(&VisibleCells::Culled(vec![1]), 0.5)
        .unwrap();

    queue_ready(&mut controller, 0);
    queue_ready(&mut controller, 2);
    queue_ready(&mut controller, 3);
    let batch = controller.take_drain_batch().unwrap();
    assert_eq!(
        batch
            .ready
            .iter()
            .map(|prepared| prepared.chunk.cluster_id)
            .collect::<Vec<_>>(),
        vec![0, 2],
        "the visible cluster's owner must not wait behind hysteresis work"
    );
}

#[test]
fn a_failure_identity_spends_only_one_leave_and_reenter_retry() {
    let mut controller = controller(topology(vec![0], vec![vec![]], vec![vec![]], vec![1]));
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), 0.0)
        .unwrap();
    controller.take_next_request().unwrap().unwrap();
    controller.mark_failed(0).unwrap();

    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), 0.1)
        .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), 2.1)
        .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), 2.2)
        .unwrap();
    assert_eq!(controller.state(0), Some(ClusterResidencyState::Absent));

    controller.take_next_request().unwrap().unwrap();
    controller.mark_failed(0).unwrap();
    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), 2.3)
        .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), 4.3)
        .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), 4.4)
        .unwrap();
    assert_eq!(controller.state(0), Some(ClusterResidencyState::Failed));
    assert!(controller.take_next_request().unwrap().is_none());
}

#[test]
fn generation_clock_exhaustion_rejects_a_new_session_without_wraparound() {
    let empty_topology = topology(Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let clock = FixedGenerationClock::new(u64::MAX);
    let controller = ShResidencyController::for_test(empty_topology, &clock).unwrap();
    assert_eq!(controller.generation(), u64::MAX);
    let next = ShResidencyController::for_test(
        topology(Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        &clock,
    );
    assert!(matches!(
        next,
        Err(ShResidencyControllerError::GenerationExhausted)
    ));
}
