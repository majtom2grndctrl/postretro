use super::super::topology::SeamPortalEndpoint;
use super::*;
use crate::sh_streaming::generation::FixedGenerationClock;
use glam::{Mat4, Vec3};
use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_loader::{CellData, CellLocatorChild, LevelWorld};
use postretro_test_log_capture::LogCapture;
use postretro_visibility::VisibleCells;
use std::sync::Arc;

#[path = "sync_manifest_test_fixture.rs"]
mod sync_manifest_test_fixture;

pub(super) fn topology(
    cell_to_cluster: Vec<u32>,
    adjacency: Vec<Vec<u32>>,
    owners: Vec<Vec<u32>>,
    requested_resident_bytes: Vec<u64>,
) -> PlannerTopology {
    hinted_topology(
        cell_to_cluster,
        adjacency,
        owners,
        requested_resident_bytes,
        Vec::new(),
        Default::default(),
        Vec::new(),
    )
}

pub(super) fn hinted_topology(
    cell_to_cluster: Vec<u32>,
    adjacency: Vec<Vec<u32>>,
    owners: Vec<Vec<u32>>,
    requested_resident_bytes: Vec<u64>,
    seam_portals: Vec<SeamPortalEndpoint>,
    pinned_clusters: std::collections::BTreeSet<u32>,
    authored_priorities: Vec<u32>,
) -> PlannerTopology {
    let cluster_count = adjacency.len();
    assert_eq!(owners.len(), cluster_count);
    assert_eq!(requested_resident_bytes.len(), cluster_count);
    let authored_priorities = if authored_priorities.is_empty() {
        vec![0; cluster_count]
    } else {
        assert_eq!(authored_priorities.len(), cluster_count);
        authored_priorities
    };
    PlannerTopology {
        cell_to_cluster,
        adjacency,
        seam_portals,
        pinned_clusters,
        authored_priorities,
        owners,
        requested_resident_bytes,
        // Distinct from `prepared`'s four decoded bytes, so tests can tell
        // encoded and decoded accounting apart.
        encoded_chunk_bytes: vec![7; cluster_count],
        chunk_hashes: (0..cluster_count)
            .map(|cluster_id| [cluster_id as u8; 32])
            .collect(),
    }
}

pub(super) fn controller(topology: PlannerTopology) -> ShResidencyController {
    ShResidencyController::for_test(topology, &FixedGenerationClock::new(1)).unwrap()
}

pub(super) fn controller_with_nominal_budget(
    topology: PlannerTopology,
    nominal_cluster_bytes: u64,
) -> ShResidencyController {
    ShResidencyController::for_test_with_budget(
        topology,
        &FixedGenerationClock::new(1),
        ShGpuBudgetInputs {
            fixed: FixedGpuCharges::default(),
            renderer_effective_floor_bytes: Some(nominal_cluster_bytes),
            ..ShGpuBudgetInputs::default()
        },
    )
    .unwrap()
}

/// Pure large-map allocation evidence. The values model the renderer report:
/// whole-load allocates every SH family, while the streamed request contains
/// fixed metadata, whole-resident billboard scatter, and one active pool.
struct LargeMapAllocationFixture {
    whole_load_requested_sh_bytes: u64,
    limited_visible_cluster_bytes: u64,
    cluster_requested_bytes: Vec<u64>,
    gpu_budget: ShGpuBudgetInputs,
}

fn large_map_allocation_fixture() -> LargeMapAllocationFixture {
    const MIB: u64 = 1024 * 1024;
    let fixed_metadata_bytes = 8 * MIB;
    let whole_resident_scatter_bytes = 16 * MIB;
    let active_pool_capacity_bytes = 240 * MIB;
    let cluster_requested_bytes = vec![64 * MIB; 8];
    LargeMapAllocationFixture {
        // The whole-load report counts every cluster plus fixed metadata and
        // the intentionally whole-resident billboard-scatter families.
        whole_load_requested_sh_bytes: cluster_requested_bytes.iter().sum::<u64>()
            + fixed_metadata_bytes
            + whole_resident_scatter_bytes,
        limited_visible_cluster_bytes: cluster_requested_bytes[0],
        cluster_requested_bytes,
        gpu_budget: ShGpuBudgetInputs {
            fixed: FixedGpuCharges {
                fixed_metadata_bytes,
                whole_resident_scatter_bytes,
                active_pool_capacity_bytes,
            },
            renderer_effective_floor_bytes: Some(
                fixed_metadata_bytes + whole_resident_scatter_bytes + active_pool_capacity_bytes,
            ),
            ..ShGpuBudgetInputs::default()
        },
    }
}

pub(super) fn prepared(controller: &ShResidencyController, cluster_id: u32) -> PreparedShCluster {
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

pub(super) fn mark_sampleable(controller: &mut ShResidencyController, cluster_id: u32) {
    controller.states[cluster_id as usize].state = ClusterResidencyState::Sampleable;
    controller
        .accounting
        .add_logical(controller.topology.requested_resident_bytes[cluster_id as usize])
        .unwrap();
}

#[derive(Debug, PartialEq, Eq)]
struct NoHintControllerTick {
    classes: Vec<(u32, TargetClass)>,
    targets: Vec<u32>,
    requests: Vec<u32>,
    suppressed: Vec<u32>,
    evictions: Vec<u32>,
}

fn no_hint_controller_tick(
    controller: &ShResidencyController,
    requests: Vec<u32>,
    evictions: Vec<u32>,
) -> NoHintControllerTick {
    NoHintControllerTick {
        classes: controller
            .states
            .iter()
            .enumerate()
            .filter_map(|(cluster_id, state)| state.class.map(|class| (cluster_id as u32, class)))
            .collect(),
        targets: controller.targets.iter().copied().collect(),
        requests,
        suppressed: controller
            .states
            .iter()
            .enumerate()
            .filter_map(|(cluster_id, state)| state.suppressed.then_some(cluster_id as u32))
            .collect(),
        evictions,
    }
}

// Slice 4 baseline: no authored hints must retain the Slice 3 policy trace.
// Generation, content tag, and section version are deliberately not observed.
#[test]
fn no_hint_controller_trace_preserves_target_request_suppression_and_eviction_order() {
    let mut controller = controller_with_nominal_budget(
        topology(
            vec![0, 1, 2, 3],
            vec![vec![1], vec![0, 2], vec![1, 3], vec![2]],
            vec![vec![], vec![], vec![], vec![]],
            vec![8; 4],
        ),
        8,
    );
    let mut trace = Vec::new();

    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let first_batch = controller.take_async_drain_batch().unwrap();
    assert!(first_batch.evictions.is_empty());
    let first_request = controller.take_next_request().unwrap().unwrap().cluster_id;
    trace.push(no_hint_controller_tick(
        &controller,
        vec![first_request],
        first_batch.evictions,
    ));

    for cluster_id in [0, 1, 2] {
        mark_sampleable(&mut controller, cluster_id);
    }
    let pressure_batch = controller.take_async_drain_batch().unwrap();
    trace.push(no_hint_controller_tick(
        &controller,
        Vec::new(),
        pressure_batch.evictions.clone(),
    ));
    controller
        .apply_drain_outcome(ShDrainOutcome {
            evicted: pressure_batch.evictions,
            ..ShDrainOutcome::default()
        })
        .unwrap();

    controller
        .update_targets(&VisibleCells::Culled(vec![3]), Some(3), 1.0)
        .unwrap();
    let recovery_batch = controller.take_async_drain_batch().unwrap();
    let mut recovery_requests = Vec::new();
    while let Some(request) = controller.take_next_request().unwrap() {
        recovery_requests.push(request.cluster_id);
    }
    trace.push(no_hint_controller_tick(
        &controller,
        recovery_requests,
        recovery_batch.evictions,
    ));

    assert_eq!(
        trace,
        vec![
            NoHintControllerTick {
                classes: vec![
                    (0, TargetClass::Visible),
                    (1, TargetClass::Prefetch),
                    (2, TargetClass::Prefetch),
                ],
                targets: vec![0, 1, 2],
                requests: vec![0],
                suppressed: Vec::new(),
                evictions: Vec::new(),
            },
            NoHintControllerTick {
                classes: vec![(0, TargetClass::Visible)],
                targets: vec![0],
                requests: Vec::new(),
                suppressed: vec![1, 2],
                evictions: vec![1, 2],
            },
            NoHintControllerTick {
                classes: vec![
                    (0, TargetClass::Hysteresis),
                    (1, TargetClass::Prefetch),
                    (2, TargetClass::Prefetch),
                    (3, TargetClass::Visible),
                ],
                targets: vec![0, 1, 2, 3],
                requests: vec![3, 1, 2],
                suppressed: Vec::new(),
                evictions: Vec::new(),
            },
        ]
    );
}

/// This uses the production portal visibility traversal with a blocked doorway
/// rather than constructing `VisibleCells` by hand. The authored seam remains
/// a planner-only overlay: the far cell is still absent from render visibility.
#[test]
fn closed_door_visibility_promotes_only_the_loader_resolved_seam_endpoint() {
    let world = LevelWorld::new_visibility_only(
        vec![
            CellData {
                bounds_min: Vec3::new(0.0, -1.0, -1.0),
                bounds_max: Vec3::new(1.0, 1.0, 1.0),
                face_start: 0,
                face_count: 1,
                portal_ref_start: 0,
                portal_ref_count: 1,
                is_solid: false,
                is_exterior: false,
                is_drawable: true,
            },
            CellData {
                bounds_min: Vec3::new(1.0, -1.0, -1.0),
                bounds_max: Vec3::new(2.0, 1.0, 1.0),
                face_start: 1,
                face_count: 1,
                portal_ref_start: 1,
                portal_ref_count: 1,
                is_solid: false,
                is_exterior: false,
                is_drawable: true,
            },
        ],
        vec![0, 0],
        CellLocatorChild::Cell(0),
        Vec::new(),
        vec![postretro_level_loader::PortalData {
            polygon: vec![
                Vec3::new(1.0, -0.5, -0.5),
                Vec3::new(1.0, 0.5, -0.5),
                Vec3::new(1.0, 0.5, 0.5),
                Vec3::new(1.0, -0.5, 0.5),
            ],
            front_cell: 0,
            back_cell: 1,
        }],
        true,
    )
    .expect("doorway visibility world must be valid");
    let eye = Vec3::new(0.25, 0.0, 0.0);
    let view_proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 16.0)
        * Mat4::look_at_rh(eye, eye + Vec3::X, Vec3::Y);
    let (visibility, _) = postretro_visibility::determine_visible_cells(
        eye,
        view_proj,
        &world,
        &[true],
        false,
        &mut Vec::new(),
    );
    assert_eq!(culled_ids(&visibility.visible_cells), &[0]);

    let mut controller = controller(hinted_topology(
        vec![0, 1],
        vec![vec![1], vec![0]],
        vec![vec![], vec![]],
        vec![4, 4],
        vec![SeamPortalEndpoint {
            portal_id: 0,
            front_cluster_id: 0,
            back_cluster_id: 1,
        }],
        Default::default(),
        vec![0, 3],
    ));
    controller
        .update_targets(
            &visibility.visible_cells,
            Some(visibility.stats.camera_cell as usize),
            0.0,
        )
        .unwrap();
    assert_eq!(controller.state(1), Some(ClusterResidencyState::Absent));
    assert_eq!(controller.states[1].class, Some(TargetClass::SeamWarm));
    assert_eq!(controller.states[1].effective_priority, 3);
    assert_eq!(culled_ids(&visibility.visible_cells), &[0]);
}

/// Cross-crate fixture proof: this PRL was baked from Task 4's committed
/// doorway source, then loaded through the public loader before the planner
/// sees its validated seam endpoints. The blocked door exercises the same
/// render-preparation visibility path used by the app.
#[test]
fn compiled_hinted_doorway_keeps_closed_visibility_and_warms_far_seam_endpoint() {
    let map_source = include_str!("../../../../content/dev/maps/sh-streaming-hinted-door.map");
    assert!(map_source.contains("\"classname\" \"streaming_seam_volume\""));
    assert!(map_source.contains("\"classname\" \"stream_resident_volume\""));
    assert!(map_source.contains("\"_stream_priority\" \"3\""));
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/maps/test-fixtures/sh-streaming-hinted-door.prl");
    let world = postretro_level_loader::load_prl(fixture.to_str().unwrap())
        .expect("committed hinted doorway PRL must load");
    let manifest = Arc::clone(
        world
            .sh_stream_manifest()
            .expect("hinted doorway must retain an SH streaming manifest"),
    );
    let seam = *manifest
        .seam_portals()
        .first()
        .expect("fixture must retain one resolved seam portal");
    let mut closed_portals = vec![false; world.portals.len()];
    for seam in manifest.seam_portals() {
        let portal_index = usize::try_from(seam.portal_id).unwrap();
        closed_portals[portal_index] = true;
    }
    assert_eq!(
        closed_portals.iter().filter(|&&closed| closed).count(),
        6,
        "fixture's doorway resolves all six authored seam portal polygons"
    );
    assert!(
        closed_portals.iter().any(|&closed| !closed),
        "the visibility proof keeps unrelated portal traversal open"
    );
    assert!(
        manifest
            .cluster_directory()
            .cluster_hints
            .iter()
            .any(|hint| hint.flags != 0),
        "fixture keeps Task 4 pin coverage while leaving the seam far side unpinned"
    );
    assert!(
        manifest
            .cluster_directory()
            .cluster_hints
            .iter()
            .any(|hint| hint.priority == 3),
        "fixture keeps Task 4 priority coverage"
    );

    let portal = &world.portals[seam.portal_id as usize];
    let near_cell = u32::try_from(portal.front_cell).unwrap();
    let far_cell = u32::try_from(portal.back_cell).unwrap();
    let near_bounds = &world.cells[near_cell as usize];
    let eye = (near_bounds.bounds_min + near_bounds.bounds_max) * 0.5;
    let portal_center = portal.polygon.iter().copied().sum::<Vec3>() / portal.polygon.len() as f32;
    let view_proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 128.0)
        * Mat4::look_at_rh(eye, portal_center, Vec3::Y);
    let visible = crate::render_preparation::VisibleRenderPreparation::for_level(
        &world,
        eye,
        view_proj,
        &closed_portals,
        false,
        &mut Vec::new(),
    )
    .visible_cells;
    assert!(culled_ids(&visible).contains(&near_cell));
    assert!(
        !culled_ids(&visible).contains(&far_cell),
        "closed doorway must not expand render VisibleCells"
    );

    assert!(
        world.cell_visibility.is_some(),
        "fixture carries id 46, so the planner runs the real warm walk"
    );
    let mut controller = ShResidencyController::with_clock(
        manifest,
        ShGpuBudgetInputs::default(),
        world.cell_visibility.as_ref(),
        &FixedGenerationClock::new(1),
    )
    .unwrap();
    let visible_before = culled_ids(&visible).to_vec();
    controller
        .update_targets(&visible, Some(near_cell as usize), 0.0)
        .unwrap();
    let far_cluster = controller.topology.cell_to_cluster[far_cell as usize];
    assert_eq!(
        controller.states[far_cluster as usize].class,
        Some(TargetClass::SeamWarm),
        "validated seam metadata warms the exact far endpoint before the door opens"
    );
    assert_eq!(
        culled_ids(&visible),
        visible_before,
        "planner never changes VisibleCells"
    );
}

#[test]
fn resident_pins_and_owner_closure_survive_empty_visibility_and_pressure() {
    let mut controller = controller_with_nominal_budget(
        hinted_topology(
            vec![0, 1],
            vec![vec![], vec![]],
            vec![vec![], vec![0]],
            vec![8, 8],
            Vec::new(),
            std::collections::BTreeSet::from([1]),
            vec![0, 3],
        ),
        1,
    );
    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 0.0)
        .unwrap();
    assert_eq!(
        controller.targets.iter().copied().collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(controller.states[0].class, Some(TargetClass::Pinned));
    assert_eq!(controller.states[1].class, Some(TargetClass::Pinned));

    mark_sampleable(&mut controller, 0);
    mark_sampleable(&mut controller, 1);
    let batch = controller.take_async_drain_batch().unwrap();
    assert!(batch.evictions.is_empty());
    assert!(controller.states.iter().all(|state| !state.suppressed));
    assert_eq!(
        controller.report_snapshot().non_evictable_overshoot_bytes,
        15,
        "pins retain their closure instead of being silently selected as victims"
    );
}

#[test]
fn seam_activation_unsuppresses_its_owner_closure_without_a_horizon_change() {
    let mut controller = controller_with_nominal_budget(
        hinted_topology(
            vec![0, 1, 2],
            vec![vec![1, 2], vec![0, 2], vec![0, 1]],
            vec![vec![], vec![2], vec![]],
            vec![4; 3],
            vec![SeamPortalEndpoint {
                portal_id: 0,
                front_cluster_id: 0,
                back_cluster_id: 1,
            }],
            Default::default(),
            vec![0, 1, 3],
        ),
        1,
    );
    controller
        .update_targets(&VisibleCells::Culled(vec![2]), Some(2), 0.0)
        .unwrap();
    assert_eq!(
        controller.last_horizon,
        std::collections::BTreeSet::from([0, 1, 2])
    );
    controller.states[1].suppressed = true;
    controller.states[2].suppressed = true;

    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 1.0)
        .unwrap();
    assert_eq!(
        controller.last_horizon,
        std::collections::BTreeSet::from([0, 1, 2])
    );
    assert_eq!(controller.states[1].class, Some(TargetClass::SeamWarm));
    assert_eq!(controller.states[1].effective_priority, 1);
    assert_eq!(controller.states[2].class, Some(TargetClass::SeamWarm));
    assert_eq!(controller.states[2].effective_priority, 3);
    assert!(!controller.states[1].suppressed);
    assert!(!controller.states[2].suppressed);
}

#[test]
fn optional_priority_orders_requests_and_pressure_before_seam_work() {
    let mut controller = controller_with_nominal_budget(
        hinted_topology(
            vec![0, 1, 2, 3],
            vec![vec![1, 2], vec![0], vec![0], vec![]],
            vec![vec![], vec![], vec![], vec![]],
            vec![4; 4],
            vec![SeamPortalEndpoint {
                portal_id: 7,
                front_cluster_id: 0,
                back_cluster_id: 3,
            }],
            Default::default(),
            vec![0, 1, 3, 0],
        ),
        4,
    );
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let mut requests = Vec::new();
    while let Some(request) = controller.take_next_request().unwrap() {
        requests.push(request.cluster_id);
    }
    assert_eq!(requests, vec![0, 3, 2, 1]);

    for cluster_id in [1, 2, 3] {
        controller.states[cluster_id].state = ClusterResidencyState::Sampleable;
        controller
            .accounting
            .add_logical(controller.topology.requested_resident_bytes[cluster_id])
            .unwrap();
    }
    let batch = controller.take_async_drain_batch().unwrap();
    assert_eq!(batch.evictions, vec![1, 2]);
    assert!(controller.states[0].class == Some(TargetClass::Visible));
}

#[test]
fn pressure_keeps_high_priority_optional_when_its_cluster_id_is_lower() {
    let mut controller = controller_with_nominal_budget(
        hinted_topology(
            vec![0, 1, 2],
            vec![vec![1, 2], vec![0], vec![0]],
            vec![vec![], vec![], vec![]],
            vec![4; 3],
            Vec::new(),
            Default::default(),
            vec![0, 3, 1],
        ),
        8,
    );
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    for cluster_id in 0..3 {
        mark_sampleable(&mut controller, cluster_id);
    }

    let batch = controller.take_async_drain_batch().unwrap();
    assert_eq!(batch.evictions, vec![2]);
    assert!(controller.targets.contains(&1));
    assert!(!controller.states[1].suppressed);
    assert!(!controller.targets.contains(&2));
    assert!(controller.states[2].suppressed);
}

#[test]
fn large_map_allocation_fixture_keeps_limited_visible_request_below_whole_load() {
    let fixture = large_map_allocation_fixture();
    let mut controller = ShResidencyController::for_test_with_budget(
        topology(
            vec![0, 1, 2, 3, 4, 5, 6, 7],
            vec![vec![]; 8],
            vec![vec![]; 8],
            fixture.cluster_requested_bytes.clone(),
        ),
        &FixedGenerationClock::new(1),
        fixture.gpu_budget,
    )
    .unwrap();

    let accounting = controller.accounting();
    let effective_floor = accounting.effective_floor_bytes().unwrap();
    let limited_visible_streamed_active_request = accounting.requested_gpu_bytes().unwrap();
    assert!(fixture.whole_load_requested_sh_bytes > effective_floor);
    assert!(limited_visible_streamed_active_request < fixture.whole_load_requested_sh_bytes);

    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let batch = controller.take_async_drain_batch().unwrap();
    assert_eq!(batch.target_reset, Some(vec![1]));
    assert!(batch.target_add.is_empty());
    assert_eq!(controller.report_snapshot().target_clusters, 1);

    mark_sampleable(&mut controller, 0);
    assert_eq!(
        controller.accounting().logical_occupancy_bytes,
        fixture.limited_visible_cluster_bytes,
        "logical occupancy remains a separate sub-ledger of the active request"
    );
}

#[test]
fn async_completion_identity_rejects_changed_hash_and_returns_departed_permit() {
    let mut controller = controller(topology(vec![0], vec![vec![]], vec![vec![]], vec![4]));
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let request = controller.take_next_request().unwrap().unwrap();
    let mut changed_hash = request;
    changed_hash.chunk_hash = [99; 32];
    assert!(!controller.matches_completion_identity(changed_hash));
    assert!(controller.matches_queued_request(request));
    assert_eq!(controller.permits_in_use(), 1);

    controller
        .update_targets(
            &VisibleCells::Culled(vec![]),
            None,
            HYSTERESIS_SECONDS + 1.0,
        )
        .unwrap();
    // The first update begins hysteresis; the second expires it.
    controller
        .update_targets(
            &VisibleCells::Culled(vec![]),
            None,
            2.0 * HYSTERESIS_SECONDS + 1.0,
        )
        .unwrap();
    assert!(!controller.matches_queued_request(request));
    assert!(!controller.admit_failed_request(request).unwrap());
    assert_eq!(controller.permits_in_use(), 0);
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

/// Produces the app's real render-preparation visibility result from a tiny
/// no-portals level rather than hand-writing `VisibleCells`. Every non-empty
/// cell contains the camera, so the fallback frustum sees the requested number
/// of drawable cells; the zero fixture is an actual non-drawable cell.
fn real_visible_cells(drawable_cell_count: usize) -> VisibleCells {
    let cell_count = drawable_cell_count.max(1);
    let cells = (0..cell_count)
        .map(|cell_index| CellData {
            bounds_min: Vec3::splat(-1.0),
            bounds_max: Vec3::splat(1.0),
            face_start: cell_index as u32,
            face_count: u32::from(drawable_cell_count != 0),
            portal_ref_start: 0,
            portal_ref_count: 0,
            is_solid: false,
            is_exterior: false,
            is_drawable: drawable_cell_count != 0,
        })
        .collect();
    let world = LevelWorld::new_visibility_only(
        cells,
        Vec::new(),
        CellLocatorChild::Cell(0),
        Vec::new(),
        Vec::new(),
        false,
    )
    .expect("visibility fixture must be valid");
    let eye = Vec3::ZERO;
    let view_proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 64.0)
        * Mat4::look_at_rh(eye, Vec3::NEG_Z, Vec3::Y);
    crate::render_preparation::VisibleRenderPreparation::for_level(
        &world,
        eye,
        view_proj,
        &[],
        false,
        &mut Vec::new(),
    )
    .visible_cells
}

fn culled_ids(visible: &VisibleCells) -> &[u32] {
    match visible {
        VisibleCells::Culled(ids) => ids,
        VisibleCells::DrawAll => panic!("real fixture must stay on the culled path"),
    }
}

// Regression: synthetic ready chunks skipped the retained-file read and all CPU phase charges.
#[test]
fn sync_proof_reads_retained_manifest_and_releases_cpu_phases_after_install() {
    let (_temp, path) = sync_manifest_test_fixture::write_one_cluster_prl();
    let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    let manifest = Arc::clone(world.sh_stream_manifest().expect("id 50 selects streaming"));
    let index = &manifest.payloads().index[0];
    let payload_len = index.payload_len;
    let decoded_bytes = index.decoded_bytes;
    let mut controller = ShResidencyController::with_clock(
        manifest,
        ShGpuBudgetInputs::default(),
        world.cell_visibility.as_ref(),
        &FixedGenerationClock::new(1),
    )
    .unwrap();

    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    assert_eq!(
        controller.read_one_sync_at_target_time().unwrap(),
        SyncReadResult::Prepared(0)
    );
    assert_eq!(controller.state(0), Some(ClusterResidencyState::Ready));
    assert_eq!(controller.permits_in_use(), 1);
    let cpu = controller.accounting().cpu;
    assert_eq!(cpu.encoded.current_bytes, 0);
    assert_eq!(cpu.encoded.high_water_bytes, payload_len);
    assert_eq!(cpu.decoding.current_bytes, 0);
    assert_eq!(cpu.decoding.high_water_bytes, decoded_bytes);
    assert_eq!(cpu.ready.current_bytes, payload_len);
    assert_eq!(cpu.ready.high_water_bytes, payload_len);

    let batch = controller.take_drain_batch().unwrap();
    assert_eq!(batch.ready.len(), 1);
    assert_eq!(batch.ready[0].chunk.cluster_id, 0);
    assert_eq!(batch.ready[0].chunk.bytes.len() as u64, payload_len);
    controller
        .apply_drain_outcome(ShDrainOutcome {
            accepted: vec![0],
            ..ShDrainOutcome::default()
        })
        .unwrap();
    assert_eq!(controller.accounting().cpu.ready.current_bytes, 0);
    assert_eq!(controller.permits_in_use(), 0);
    assert_eq!(
        controller.state(0),
        Some(ClusterResidencyState::InstalledUncomposed)
    );
    controller.promote_composed_clusters();
    assert_eq!(controller.state(0), Some(ClusterResidencyState::Sampleable));
    assert_eq!(
        controller.read_one_sync_at_target_time().unwrap(),
        SyncReadResult::NoTargetReady
    );
}

#[test]
fn real_visibility_zero_one_many_targets_and_next_frame_promotion() {
    let zero = real_visible_cells(0);
    let one = real_visible_cells(1);
    let many = real_visible_cells(3);
    assert!(culled_ids(&zero).is_empty());
    assert_eq!(culled_ids(&one), &[0]);
    assert_eq!(culled_ids(&many), &[0, 1, 2]);

    let mut one_controller = controller(topology(
        vec![0, 1, 2],
        vec![vec![], vec![], vec![]],
        vec![vec![], vec![], vec![]],
        vec![4, 8, 16],
    ));
    one_controller.update_targets(&zero, None, 0.0).unwrap();
    assert_eq!(
        one_controller.take_drain_batch().unwrap().target_reset,
        Some(vec![0]),
        "the real zero-visible fixture clears all three cluster targets"
    );

    one_controller.update_targets(&one, Some(0), 1.0).unwrap();
    assert!(one_controller.is_targeted(0));
    assert!(!one_controller.is_targeted(1));
    queue_ready(&mut one_controller, 0);
    let frame_n = one_controller.take_drain_batch().unwrap();
    assert_eq!(
        frame_n
            .ready
            .iter()
            .map(|prepared| prepared.chunk.cluster_id)
            .collect::<Vec<_>>(),
        vec![0]
    );
    one_controller
        .apply_drain_outcome(ShDrainOutcome {
            accepted: vec![0],
            ..ShDrainOutcome::default()
        })
        .unwrap();
    assert_eq!(
        one_controller.state(0),
        Some(ClusterResidencyState::InstalledUncomposed),
        "frame N install cannot sample before its compose submission"
    );
    one_controller.promote_composed_clusters();
    assert_eq!(
        one_controller.state(0),
        Some(ClusterResidencyState::Sampleable),
        "frame N+1 pre-compose promotion exposes the accepted cluster"
    );

    one_controller.update_targets(&many, Some(0), 2.0).unwrap();
    assert!(one_controller.is_targeted(0));
    assert!(one_controller.is_targeted(1));
    assert!(one_controller.is_targeted(2));

    // A fresh real-many session proves the renderer handoff without letting
    // the one-visible frame above pre-satisfy cluster zero.
    let mut many_controller = controller(topology(
        vec![0, 1, 2],
        vec![vec![], vec![], vec![]],
        vec![vec![], vec![], vec![]],
        vec![4, 8, 16],
    ));
    many_controller.update_targets(&many, Some(0), 0.0).unwrap();
    queue_ready(&mut many_controller, 0);
    queue_ready(&mut many_controller, 1);
    queue_ready(&mut many_controller, 2);
    let many_frame_n = many_controller.take_drain_batch().unwrap();
    let frame_n_accepted: Vec<_> = many_frame_n
        .ready
        .iter()
        .map(|prepared| prepared.chunk.cluster_id)
        .collect();
    assert_eq!(
        frame_n_accepted,
        vec![0, 1, 2],
        "three small chunks fit one drain's decoded-byte budget"
    );
    many_controller
        .apply_drain_outcome(ShDrainOutcome {
            accepted: frame_n_accepted.clone(),
            ..ShDrainOutcome::default()
        })
        .unwrap();
    assert!(frame_n_accepted.iter().all(|&cluster_id| {
        many_controller.state(cluster_id) == Some(ClusterResidencyState::InstalledUncomposed)
    }));
    assert!(!many_controller.all_targets_sampleable());
    many_controller.promote_composed_clusters();
    assert!(many_controller.all_targets_sampleable());
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
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    assert!(controller.is_targeted(0));
    assert!(controller.is_targeted(1));
    assert!(controller.is_targeted(2));
    assert!(!controller.is_targeted(3));

    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 0.5)
        .unwrap();
    assert!(controller.is_targeted(0));
    assert!(controller.is_targeted(1));
    assert!(controller.is_targeted(2));

    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 2.49)
        .unwrap();
    assert!(controller.is_targeted(0));
    assert!(controller.is_targeted(1));
    assert!(controller.is_targeted(2));

    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 2.5)
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
        .update_targets(&VisibleCells::Culled(vec![1]), Some(1), 0.0)
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
    assert!(
        !controller.all_targets_sampleable(),
        "an accepted cluster remains unavailable until a successful compose frame completes"
    );
    controller.promote_composed_clusters();
    assert!(
        !controller.all_targets_sampleable(),
        "the halo remains targeted and is not yet sampleable"
    );

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
    controller
        .apply_drain_outcome(ShDrainOutcome {
            accepted: vec![1],
            ..ShDrainOutcome::default()
        })
        .unwrap();
    assert!(!controller.all_targets_sampleable());
    controller.promote_composed_clusters();
    assert!(controller.all_targets_sampleable());
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
        .update_targets(&VisibleCells::Culled(vec![1]), Some(1), 0.0)
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
    empty
        .update_targets(&VisibleCells::DrawAll, None, 0.0)
        .unwrap();
    assert_eq!(
        empty.take_drain_batch().unwrap().target_reset,
        Some(Vec::new())
    );

    let mut one = controller(topology(vec![0], vec![vec![]], vec![vec![]], vec![32]));
    one.update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
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
    many.update_targets(&VisibleCells::DrawAll, None, 0.0)
        .unwrap();
    let reset = many.take_drain_batch().unwrap();
    assert_eq!(reset.target_reset, Some(vec![0b111]));
    queue_ready(&mut many, 0);
    queue_ready(&mut many, 1);
    queue_ready(&mut many, 2);
    let batch = many.take_drain_batch().unwrap();
    assert_eq!(batch.ready.len(), 3);
    assert_eq!(many.permits_in_use(), 3);
}

#[test]
fn old_generation_completion_cannot_consume_a_new_request_permit() {
    let mut controller = controller(topology(vec![0], vec![vec![]], vec![vec![]], vec![1]));
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
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
fn late_completion_after_hysteresis_is_dropped_before_renderer_install() {
    let mut controller = controller(topology(vec![0], vec![vec![]], vec![vec![]], vec![4]));
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let request = controller
        .take_next_request()
        .unwrap()
        .expect("visible cluster must reserve a lifecycle permit");
    assert_eq!(request.cluster_id, 0);
    assert_eq!(controller.permits_in_use(), 1);

    // The queued request outlives the two-second retention window. It is
    // intentionally not cancelled in-place: a worker may still complete it,
    // so admission must perform the definitive target check.
    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 0.1)
        .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 2.2)
        .unwrap();
    assert!(!controller.is_targeted(0));
    assert_eq!(controller.state(0), Some(ClusterResidencyState::Queued));
    assert_eq!(controller.accounting().cpu.ready.current_bytes, 0);

    assert_eq!(
        controller.admit_prepared(prepared(&controller, 0)).unwrap(),
        ShDrainAdmission::DroppedNotTargeted,
        "a completion that arrives after its target horizon departed cannot reach the renderer"
    );
    assert_eq!(controller.permits_in_use(), 0);
    assert_eq!(controller.accounting().cpu.ready.current_bytes, 0);
    assert_eq!(controller.state(0), Some(ClusterResidencyState::Absent));
    assert!(
        controller.take_drain_batch().unwrap().ready.is_empty(),
        "a late completion never becomes an install batch entry"
    );
}

#[test]
fn lifecycle_permits_bound_queued_work() {
    let cluster_count = MAX_STREAM_PERMITS + 1;
    let mut controller = controller(topology(
        (0..cluster_count as u32).collect(),
        vec![vec![]; cluster_count],
        vec![vec![]; cluster_count],
        vec![1; cluster_count],
    ));
    controller
        .update_targets(&VisibleCells::DrawAll, None, 0.0)
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

// Regression: an owner left on the walk's path by an early return read as a
// cycle when a sibling branch reached it again (a diamond, not a cycle).
#[test]
fn owner_walk_revisiting_a_blocked_owner_is_not_a_cycle() {
    let mut controller = controller(topology(
        vec![0, 1, 2, 3],
        vec![vec![], vec![], vec![], vec![]],
        vec![vec![1, 2], vec![3], vec![1], vec![]],
        vec![1; 4],
    ));
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    assert_eq!(
        controller
            .take_next_request()
            .unwrap()
            .map(|r| r.cluster_id),
        Some(3)
    );
    // Cluster 1 is now blocked behind queued 3; cluster 2 reaches 1 again.
    controller
        .take_next_request()
        .expect("an acyclic owner graph never reports a cycle");
}

// Regression: an absent owner blocked behind a queued grand-owner let the walk
// fall through and request the dependent ahead of its owner.
#[test]
fn owner_blocked_behind_in_flight_work_defers_its_dependents() {
    let mut controller = controller(topology(
        vec![0, 1, 2, 3],
        vec![vec![], vec![], vec![], vec![]],
        vec![vec![1, 2], vec![3], vec![1], vec![]],
        vec![1; 4],
    ));
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    assert_eq!(
        controller
            .take_next_request()
            .unwrap()
            .map(|r| r.cluster_id),
        Some(3)
    );
    assert_eq!(
        controller
            .take_next_request()
            .unwrap()
            .map(|r| r.cluster_id),
        None,
        "clusters 0, 1, and 2 all wait on queued owner 3"
    );
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
        .update_targets(&VisibleCells::DrawAll, None, 0.0)
        .unwrap();
    controller.take_drain_batch().unwrap();
    controller
        .update_targets(&VisibleCells::Culled(vec![1]), Some(1), 0.5)
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
        vec![0, 2, 3],
        "the visible cluster's owner must not wait behind hysteresis work"
    );
}

#[test]
fn a_failure_identity_warns_once_and_spends_only_one_leave_and_reenter_retry() {
    let mut controller = controller(topology(vec![0], vec![vec![]], vec![vec![]], vec![1]));
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let first = controller.take_next_request().unwrap().unwrap();
    assert!(controller.admit_failed_request(first).unwrap());

    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 0.1)
        .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 2.1)
        .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 2.2)
        .unwrap();
    assert_eq!(controller.state(0), Some(ClusterResidencyState::Absent));

    let retry = controller.take_next_request().unwrap().unwrap();
    assert_eq!(retry, first);
    assert!(!controller.admit_failed_request(retry).unwrap());
    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 2.3)
        .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 4.3)
        .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 4.4)
        .unwrap();
    assert_eq!(controller.state(0), Some(ClusterResidencyState::Failed));
    assert!(controller.take_next_request().unwrap().is_none());
}

#[test]
fn failure_warning_resets_when_hash_generation_or_content_tag_identity_changes() {
    let mut controller = controller(topology(vec![0], vec![vec![]], vec![vec![]], vec![1]));
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();

    let first = controller.take_next_request().unwrap().unwrap();
    assert!(controller.admit_failed_request(first).unwrap());

    controller.states[0].state = ClusterResidencyState::Absent;
    let repeated = controller.take_next_request().unwrap().unwrap();
    assert_eq!(repeated, first);
    assert!(!controller.admit_failed_request(repeated).unwrap());

    controller.topology.chunk_hashes[0] = [9; 32];
    controller.states[0].state = ClusterResidencyState::Absent;
    let changed_hash = controller.take_next_request().unwrap().unwrap();
    assert_ne!(changed_hash.chunk_hash, first.chunk_hash);
    assert!(controller.admit_failed_request(changed_hash).unwrap());

    controller.generation += 1;
    controller.states[0].state = ClusterResidencyState::Absent;
    let changed_generation = controller.take_next_request().unwrap().unwrap();
    assert_ne!(changed_generation.generation, changed_hash.generation);
    assert!(controller.admit_failed_request(changed_generation).unwrap());

    controller.content_tag = [7; 32];
    controller.states[0].state = ClusterResidencyState::Absent;
    let changed_content_tag = controller.take_next_request().unwrap().unwrap();
    assert_ne!(
        changed_content_tag.content_tag,
        changed_generation.content_tag
    );
    assert_eq!(
        changed_content_tag.generation,
        changed_generation.generation
    );
    assert_eq!(
        changed_content_tag.chunk_hash,
        changed_generation.chunk_hash
    );
    assert!(
        controller
            .admit_failed_request(changed_content_tag)
            .unwrap()
    );
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

#[test]
fn departed_residents_wait_for_hysteresis_then_evict_dependents_before_owners() {
    let mut controller = controller_with_nominal_budget(
        topology(
            vec![0, 1],
            vec![vec![], vec![]],
            vec![vec![], vec![0]],
            vec![8, 8],
        ),
        64,
    );
    controller
        .update_targets(&VisibleCells::Culled(vec![1]), Some(1), 0.0)
        .unwrap();
    let _ = controller.take_async_drain_batch().unwrap();
    mark_sampleable(&mut controller, 0);
    mark_sampleable(&mut controller, 1);

    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 0.1)
        .unwrap();
    assert!(
        controller
            .take_async_drain_batch()
            .unwrap()
            .evictions
            .is_empty(),
        "two-second retention keeps doorway departures resident"
    );
    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 2.1)
        .unwrap();
    let batch = controller.take_async_drain_batch().unwrap();
    assert_eq!(batch.target_remove, vec![0, 1]);
    assert_eq!(
        batch.evictions,
        vec![1, 0],
        "a dependent must leave before its baked owner"
    );

    controller
        .apply_drain_outcome(ShDrainOutcome {
            evicted: vec![0, 1],
            ..ShDrainOutcome::default()
        })
        .unwrap();
    assert_eq!(controller.accounting().logical_occupancy_bytes, 0);
    assert_eq!(controller.counters().evictions, 2);
}

#[test]
fn sync_proof_drain_keeps_departed_residents_without_budget_eviction() {
    let mut controller =
        controller_with_nominal_budget(topology(vec![0], vec![vec![]], vec![vec![]], vec![12]), 8);
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let _ = controller.take_drain_batch().unwrap();
    mark_sampleable(&mut controller, 0);

    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 0.1)
        .unwrap();
    let _ = controller.take_drain_batch().unwrap();
    controller
        .update_targets(&VisibleCells::Culled(Vec::new()), None, 2.2)
        .unwrap();
    let batch = controller.take_drain_batch().unwrap();

    assert_eq!(batch.target_remove, vec![0]);
    assert!(batch.evictions.is_empty());
    assert_eq!(controller.state(0), Some(ClusterResidencyState::Sampleable));
    assert_eq!(controller.accounting().logical_occupancy_bytes, 12);
    assert_eq!(controller.non_evictable_overshoot_bytes(), 0);
}

#[test]
fn pressure_suppresses_prefetch_persistently_without_evicting_visible_work() {
    let mut controller = controller_with_nominal_budget(
        topology(
            vec![0, 1, 2, 3],
            vec![vec![1], vec![0, 2], vec![1, 3], vec![2]],
            vec![vec![], vec![], vec![], vec![]],
            vec![8, 8, 8, 8],
        ),
        8,
    );
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let _ = controller.take_async_drain_batch().unwrap();
    mark_sampleable(&mut controller, 0);
    mark_sampleable(&mut controller, 1);
    mark_sampleable(&mut controller, 2);

    let pressure = controller.take_async_drain_batch().unwrap();
    assert_eq!(pressure.target_remove, vec![1, 2]);
    assert_eq!(pressure.evictions, vec![1, 2]);
    assert!(controller.is_targeted(0));
    assert!(!controller.is_targeted(1));
    assert!(!controller.is_targeted(2));
    controller
        .apply_drain_outcome(ShDrainOutcome {
            evicted: vec![1, 2],
            ..ShDrainOutcome::default()
        })
        .unwrap();

    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 1.0)
        .unwrap();
    assert!(
        !controller.is_targeted(1),
        "pressure suppression survives an unchanged two-hop horizon"
    );
    controller
        .update_targets(&VisibleCells::Culled(vec![3]), Some(3), 2.0)
        .unwrap();
    assert!(
        controller.is_targeted(1),
        "a changed horizon clears suppression so the prefetch can recover"
    );
    assert!(controller.is_targeted(3));
}

// Regression: raw horizon bytes cleared suppression while its owner closure remained over budget.
#[test]
fn pressure_recovery_waits_until_owner_closed_horizon_fits() {
    let mut controller = ShResidencyController::for_test_with_budget(
        topology(
            vec![0, 1, 2],
            vec![vec![1], vec![0], vec![]],
            vec![vec![], vec![2], vec![]],
            vec![8, 8, 8],
        ),
        &FixedGenerationClock::new(1),
        ShGpuBudgetInputs {
            fixed: FixedGpuCharges {
                fixed_metadata_bytes: 8,
                ..FixedGpuCharges::default()
            },
            renderer_effective_floor_bytes: Some(24),
            ..ShGpuBudgetInputs::default()
        },
    )
    .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let _ = controller.take_async_drain_batch().unwrap();
    mark_sampleable(&mut controller, 0);
    mark_sampleable(&mut controller, 1);
    mark_sampleable(&mut controller, 2);

    let pressure = controller.take_async_drain_batch().unwrap();
    assert_eq!(pressure.target_remove, vec![1]);
    assert_eq!(pressure.evictions, vec![1]);
    controller
        .apply_drain_outcome(ShDrainOutcome {
            evicted: vec![1],
            ..ShDrainOutcome::default()
        })
        .unwrap();

    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 1.0)
        .unwrap();
    assert!(!controller.is_targeted(1));
    assert!(!controller.is_targeted(2));
    assert!(controller.take_next_request().unwrap().is_none());

    controller
        .update_gpu_charges(FixedGpuCharges::default())
        .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 2.0)
        .unwrap();
    assert!(controller.is_targeted(1));
    assert!(controller.is_targeted(2));
    assert_eq!(
        controller.take_next_request().unwrap().unwrap().cluster_id,
        1
    );
}

#[test]
fn pressure_rechecks_a_prefetch_owner_after_its_prefetch_dependent_is_suppressed() {
    let mut controller = controller_with_nominal_budget(
        topology(
            vec![0, 1, 2],
            vec![vec![1], vec![0, 2], vec![1]],
            vec![vec![], vec![2], vec![]],
            vec![8, 8, 8],
        ),
        8,
    );
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let _ = controller.take_async_drain_batch().unwrap();
    mark_sampleable(&mut controller, 0);
    mark_sampleable(&mut controller, 1);
    mark_sampleable(&mut controller, 2);

    let batch = controller.take_async_drain_batch().unwrap();
    assert_eq!(batch.target_remove, vec![1, 2]);
    assert_eq!(batch.evictions, vec![1, 2]);
    assert!(controller.is_targeted(0));
    assert!(!controller.is_targeted(1));
    assert!(!controller.is_targeted(2));
    assert_eq!(controller.non_evictable_overshoot_bytes(), 0);
}

#[test]
fn pressure_owner_recheck_does_not_log_a_transient_overshoot() {
    let capture = LogCapture::start();
    let mut controller = controller_with_nominal_budget(
        topology(
            vec![0, 1, 2],
            vec![vec![1], vec![0, 2], vec![1]],
            vec![vec![], vec![2], vec![]],
            vec![8, 8, 8],
        ),
        8,
    );
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    for cluster_id in 0..3 {
        mark_sampleable(&mut controller, cluster_id);
    }

    let batch = controller.take_async_drain_batch().unwrap();
    assert_eq!(batch.evictions, vec![1, 2]);
    assert_eq!(
        controller.report_snapshot().non_evictable_overshoot_bytes,
        0
    );
    capture.assert_not_logged(
        log::Level::Warn,
        "non-evictable logical demand exceeds the effective floor",
    );
}

#[test]
fn pressure_does_not_evict_a_just_installed_prefetch_cluster() {
    let mut controller = controller_with_nominal_budget(
        topology(
            vec![0, 1],
            vec![vec![1], vec![0]],
            vec![vec![], vec![]],
            vec![8, 8],
        ),
        8,
    );
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let _ = controller.take_async_drain_batch().unwrap();
    mark_sampleable(&mut controller, 0);
    controller.states[1].state = ClusterResidencyState::InstalledUncomposed;
    controller.accounting.add_logical(8).unwrap();

    let batch = controller.take_async_drain_batch().unwrap();
    assert!(controller.is_targeted(1));
    assert!(batch.target_remove.is_empty());
    assert!(batch.evictions.is_empty());
}

#[test]
fn non_evictable_overshoot_logs_once_per_onset_and_remains_separate_from_replacement_peak() {
    let capture = LogCapture::start();
    let mut controller =
        controller_with_nominal_budget(topology(vec![0], vec![vec![]], vec![vec![]], vec![12]), 8);
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();
    let _ = controller.take_async_drain_batch().unwrap();
    let _ = controller.take_async_drain_batch().unwrap();

    assert_eq!(controller.non_evictable_overshoot_bytes(), 4);
    capture.assert_logged_once(
        log::Level::Warn,
        "non-evictable logical demand exceeds the effective floor by 4 bytes",
    );
    let snapshot = controller.report_snapshot();
    assert_eq!(snapshot.non_evictable_overshoot_bytes, 4);
    assert_eq!(snapshot.target_clusters, 1);
    assert_eq!(snapshot.counters.misses, 1);
}

#[test]
fn a_pinned_prefetch_owner_counts_as_non_evictable_overshoot() {
    let mut controller = controller_with_nominal_budget(
        topology(
            vec![0, 1, 2],
            vec![vec![1], vec![0, 2], vec![1]],
            vec![vec![], vec![2], vec![]],
            vec![8, 8, 8],
        ),
        8,
    );
    controller
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
        .unwrap();

    let _ = controller.take_async_drain_batch().unwrap();
    assert_eq!(controller.non_evictable_overshoot_bytes(), 8);
}

#[test]
fn outcome_preflight_does_not_evict_before_later_install_counter_overflow() {
    let mut controller = controller_with_nominal_budget(
        topology(
            vec![0, 1],
            vec![vec![], vec![]],
            vec![vec![], vec![]],
            vec![4, 4],
        ),
        16,
    );
    controller
        .update_targets(&VisibleCells::Culled(vec![0, 1]), Some(0), 0.0)
        .unwrap();
    let _ = controller.take_async_drain_batch().unwrap();
    mark_sampleable(&mut controller, 0);
    queue_ready(&mut controller, 1);
    controller
        .update_targets(&VisibleCells::Culled(vec![1]), Some(1), 0.1)
        .unwrap();
    controller
        .update_targets(&VisibleCells::Culled(vec![1]), Some(1), 2.2)
        .unwrap();
    let batch = controller.take_async_drain_batch().unwrap();
    assert_eq!(batch.evictions, vec![0]);
    assert_eq!(
        batch
            .ready
            .iter()
            .map(|prepared| prepared.chunk.cluster_id)
            .collect::<Vec<_>>(),
        vec![1]
    );
    controller.counters.installs = u64::MAX;

    assert!(matches!(
        controller.apply_drain_outcome(ShDrainOutcome {
            accepted: vec![1],
            evicted: vec![0],
            ..ShDrainOutcome::default()
        }),
        Err(ShResidencyControllerError::AccountingOverflow(
            "stream installs"
        ))
    ));
    assert_eq!(controller.state(0), Some(ClusterResidencyState::Sampleable));
    assert_eq!(controller.accounting().logical_occupancy_bytes, 4);
    assert_eq!(controller.counters.evictions, 0);
    assert!(controller.in_drain.contains_key(&1));
}
