//! SH streaming regression baseline, controller half: drives the controller
//! over a fixed synthetic frame schedule and compares what it did against a
//! committed trace. Recorded before the shared streaming layer was extracted;
//! the extraction must replay it unchanged (brief AC 18). The issuer half is
//! `session/sh_async_workers/issuer_trace_tests.rs`.
//!
//! The driver follows the production async frame (`LevelStreaming::prepare_drains`
//! with SH alone): promote last frame's installs, update targets, admit last
//! frame's completions, drain plus requests, then apply the renderer's
//! outcome. A simulated issuer completes every outstanding request next
//! frame. It cancels one whose cluster left the target set, unless the
//! schedule marked that read as already in flight; in-flight reads complete
//! as prepared. The simulated renderer owns a pool of whole clusters: it
//! confirms every eviction, walks the victims to seat mandatory work, grows
//! once they run out, and refuses optional work that does not fit.
//!
//! Regenerate only when a behaviour change is intended:
//! `POSTRETRO_REGEN_SH_TRACE=1 cargo test -p postretro --bin postretro sh_trace`.
//! See: context/lib/rendering_pipeline.md §4

use std::collections::BTreeSet;
use std::fmt::Write as _;

use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_visibility::VisibleCells;

use super::super::topology::SeamPortalEndpoint;
use super::tests::hinted_topology;
use super::*;
use crate::lightmap_streaming::test_fixtures::{PORTAL, residency_set};
use crate::sh_streaming::generation::FixedGenerationClock;
use crate::sh_streaming::trace_fixture::assert_matches_baseline;
use crate::streaming::cell_demand::CellDemand;
use crate::streaming::drain_budget::MAX_INSTALL_DECODED_BYTES_PER_DRAIN;

const BASELINE: &str = "sh_controller_trace_baseline.txt";
const MIB: usize = 1024 * 1024;
const CLUSTERS: usize = 12;
/// Decoded chunk size per cluster. Mixed so drains both fill and stop on the
/// 8 MiB budget; cluster 6 alone exceeds it.
const DECODED_MIB: [usize; CLUSTERS] = [2, 1, 3, 5, 1, 1, 9, 2, 1, 3, 1, 2];
/// The simulated pool: seven clusters, below the reach plus the pin, so
/// optional work yields to mandatory work.
const POOL_CLUSTERS: usize = 7;

/// One render frame of the schedule.
struct Frame {
    seconds: f64,
    camera_cell: usize,
    visible_cells: &'static [u32],
    /// Outstanding reads already in flight: they do not complete this frame
    /// and, once they do, complete prepared even if their cluster has left.
    in_flight: &'static [u32],
}

const fn frame(
    seconds: f64,
    camera_cell: usize,
    visible_cells: &'static [u32],
    in_flight: &'static [u32],
) -> Frame {
    Frame {
        seconds,
        camera_cell,
        visible_cells,
        in_flight,
    }
}

/// Cells 0..11 in a chain, four metres a hop, one cluster per cell. Cluster
/// 11 is pinned; an authored seam joins 6 and 9; cluster 7 (priority 3) and 9 (priority 2) carry authored
/// priority.
const SCHEDULE: &[Frame] = &[
    // Spawn: visible 0-1, lead 0..4, band 5..6, pinned 11. Permits cap the
    // first requests.
    frame(0.0, 0, &[0, 1], &[]),
    frame(0.1, 0, &[0, 1], &[]),
    frame(0.2, 0, &[0, 1], &[]),
    // Seam near side 6 visible from cell 3: 9, also in the band, is seam-warm.
    frame(0.3, 3, &[3, 6], &[]),
    // The seam closes as 9's read lands. Mandatory 5 and 7 outgrow the pool
    // with no victim to take, and the full pool refuses 9, now band work.
    frame(0.4, 3, &[3], &[]),
    // The seam reopens: 9 is newly seam-warm, which lifts its suppression, and
    // the pool refuses band 8.
    frame(0.5, 3, &[3, 6], &[]),
    // Closes again with 9's read in flight. It lands as band work next frame,
    // and the pool refuses it.
    frame(0.6, 3, &[3], &[9]),
    frame(0.7, 3, &[3], &[]),
    // The reach covers the whole chain from the middle. Mandatory 8, 9 and 10
    // take the room band clusters 0 and 1 held, then grow the pool by one.
    frame(1.0, 6, &[6, 7], &[]),
    frame(1.1, 6, &[6, 7], &[]),
    frame(1.2, 6, &[6, 7], &[]),
    // Far end: reach 4..11. 0..3 leave the horizon and enter hysteresis; 0 and
    // 1, evicted earlier, are read again as hysteresis targets and refused.
    frame(3.5, 10, &[10, 11], &[]),
    frame(3.6, 10, &[10, 11], &[]),
    frame(3.7, 10, &[10, 11], &[]),
    // 0..3 have outlived hysteresis.
    frame(6.0, 10, &[10], &[]),
    frame(6.1, 10, &[10], &[]),
];

/// The chain's id 51: from each camera cell, every cell within six hops at
/// four metres a hop. Cells within 16 m (four hops) are lead, the rest band.
fn chain_set() -> CellResidencySetSection {
    let rows: Vec<(u32, u32, u32)> = (0..CLUSTERS as u32)
        .flat_map(|camera| {
            (0..CLUSTERS as u32)
                .filter(move |&cell| camera.abs_diff(cell) <= 6)
                .map(move |cell| (camera, cell, camera.abs_diff(cell) * 4))
        })
        .collect();
    residency_set(CLUSTERS as u32, &rows, 32)
}

fn trace_controller() -> ShResidencyController {
    let chain: Vec<Vec<u32>> = (0..CLUSTERS as u32)
        .map(|cluster| {
            [cluster.checked_sub(1), Some(cluster + 1)]
                .into_iter()
                .flatten()
                .filter(|&neighbor| (neighbor as usize) < CLUSTERS)
                .collect()
        })
        .collect();
    let mut priorities = vec![0; CLUSTERS];
    priorities[7] = 3;
    priorities[9] = 2;
    ShResidencyController::for_test(
        hinted_topology(
            (0..CLUSTERS as u32).collect(),
            chain,
            vec![SeamPortalEndpoint {
                portal_id: 0,
                front_cluster_id: 6,
                back_cluster_id: 9,
            }],
            BTreeSet::from([11]),
            priorities,
        ),
        &FixedGenerationClock::new(1),
    )
    .unwrap()
}

/// The renderer's half of a drain over a pool of whole clusters: `install`
/// order, the victims walked only as far as mandatory work needs.
#[derive(Default)]
struct SimulatedPool {
    residents: BTreeSet<u32>,
    capacity: usize,
}

impl SimulatedPool {
    fn drain(&mut self, batch: &ShDrainBatch) -> ShDrainOutcome {
        let mut outcome = ShDrainOutcome {
            evicted: batch.evictions.clone(),
            ..ShDrainOutcome::default()
        };
        for cluster_id in &batch.evictions {
            self.residents.remove(cluster_id);
        }
        let mut victims = batch.victims.iter().copied();
        for prepared in &batch.ready {
            let cluster_id = prepared.chunk.cluster_id;
            while self.residents.len() >= self.capacity
                && batch.mandatory.binary_search(&cluster_id).is_ok()
            {
                let Some(victim) = victims.next() else {
                    self.capacity = batch.mandatory.len().max(self.residents.len() + 1);
                    break;
                };
                self.residents.remove(&victim);
                outcome.evicted.push(victim);
            }
            if self.residents.len() < self.capacity {
                self.residents.insert(cluster_id);
                outcome.accepted.push(cluster_id);
            } else {
                outcome.pool_refused.push(cluster_id);
            }
        }
        outcome
    }
}

fn class_name(class: TargetClass) -> &'static str {
    match class {
        TargetClass::Visible => "visible",
        TargetClass::Pinned => "pinned",
        TargetClass::Lead => "lead",
        TargetClass::SeamWarm => "seam-warm",
        TargetClass::Band => "band",
        TargetClass::Hysteresis => "hysteresis",
        TargetClass::Held => "held",
    }
}

fn admission_name(admission: ShDrainAdmission) -> &'static str {
    match admission {
        ShDrainAdmission::Ready => "ready",
        ShDrainAdmission::DroppedStale => "dropped-stale",
        ShDrainAdmission::DroppedNotTargeted => "dropped-not-targeted",
        ShDrainAdmission::DroppedDuplicate => "dropped-duplicate",
    }
}

fn ids(ids: impl IntoIterator<Item = u32>) -> String {
    let ids: Vec<String> = ids.into_iter().map(|id| id.to_string()).collect();
    format!("[{}]", ids.join(" "))
}

fn decoded_chunk(controller: &ShResidencyController, cluster_id: u32) -> PreparedShCluster {
    PreparedShCluster {
        generation: controller.generation(),
        content_tag: controller.content_tag(),
        chunk: DecodedClusterShPayload {
            cluster_id,
            bytes: vec![cluster_id as u8; DECODED_MIB[cluster_id as usize] * MIB],
            blocks: Vec::new(),
        },
    }
}

/// Completes last frame's outstanding requests as the issuer would have.
fn complete_outstanding(
    controller: &mut ShResidencyController,
    frame: &Frame,
    outstanding: &mut Vec<ShClusterRequest>,
    reading: &mut BTreeSet<u32>,
    out: &mut String,
) {
    reading.extend(frame.in_flight.iter().copied());
    let mut events = Vec::new();
    let mut still_outstanding = Vec::new();
    for request in outstanding.drain(..) {
        let cluster_id = request.cluster_id;
        if frame.in_flight.contains(&cluster_id) {
            events.push(format!("{cluster_id} in-flight"));
            still_outstanding.push(request);
        } else if reading.remove(&cluster_id) || controller.targets().contains(&cluster_id) {
            let admission = controller
                .admit_prepared(decoded_chunk(controller, cluster_id))
                .unwrap();
            events.push(format!("{cluster_id} {}", admission_name(admission)));
        } else {
            controller.admit_cancelled_request(request).unwrap();
            events.push(format!("{cluster_id} cancelled"));
        }
    }
    *outstanding = still_outstanding;
    if !events.is_empty() {
        writeln!(out, "  completions: {}", events.join(", ")).unwrap();
    }
}

fn record_drain(controller: &ShResidencyController, batch: &ShDrainBatch, out: &mut String) {
    if let Some(reset) = &batch.target_reset {
        let reset_ids = (0..CLUSTERS as u32)
            .filter(|&id| reset[id as usize / 64] & (1 << (id % 64)) != 0)
            .collect::<Vec<_>>();
        writeln!(out, "  drain target_reset={}", ids(reset_ids)).unwrap();
    } else {
        writeln!(
            out,
            "  drain target_add={} target_remove={}",
            ids(batch.target_add.iter().copied()),
            ids(batch.target_remove.iter().copied())
        )
        .unwrap();
    }
    if !batch.evictions.is_empty() {
        writeln!(out, "  evict: {}", ids(batch.evictions.iter().copied())).unwrap();
    }
    if !batch.victims.is_empty() {
        writeln!(out, "  victims: {}", ids(batch.victims.iter().copied())).unwrap();
    }
    let admitted: Vec<String> = batch
        .ready
        .iter()
        .map(|prepared| {
            format!(
                "{} {}B",
                prepared.chunk.cluster_id,
                prepared.chunk.bytes.len()
            )
        })
        .collect();
    let admitted_bytes: usize = batch
        .ready
        .iter()
        .map(|prepared| prepared.chunk.bytes.len())
        .sum();
    writeln!(
        out,
        "  install: [{}] = {admitted_bytes}B of {MAX_INSTALL_DECODED_BYTES_PER_DRAIN}B",
        admitted.join(", ")
    )
    .unwrap();
    // Ready work the byte budget left behind, in cluster-id order.
    let left: Vec<String> = controller
        .ready
        .iter()
        .map(|(&cluster_id, ready)| format!("{cluster_id} {}B budget", ready.byte_charge))
        .collect();
    if !left.is_empty() {
        writeln!(out, "  ready left: [{}]", left.join(", ")).unwrap();
    }
}

fn record_state(controller: &ShResidencyController, out: &mut String) {
    let targets: Vec<String> = controller
        .targets()
        .iter()
        .map(|&cluster_id| {
            let state = &controller.states[cluster_id as usize];
            let class = state.class.map_or("none", class_name);
            if state.effective_priority == 0 {
                format!("{cluster_id} {class}")
            } else {
                format!("{cluster_id} {class} p{}", state.effective_priority)
            }
        })
        .collect();
    writeln!(out, "  targets: [{}]", targets.join(", ")).unwrap();
    let suppressed = controller
        .states
        .iter()
        .enumerate()
        .filter_map(|(cluster_id, state)| state.suppressed.then_some(cluster_id as u32));
    let suppressed = ids(suppressed);
    if suppressed != "[]" {
        writeln!(out, "  suppressed: {suppressed}").unwrap();
    }
    let counters = controller.counters();
    writeln!(
        out,
        "  counters: installs={} evictions={} cancelled={} discarded_reads={} \
         decoded_installed={}B budget_limited_drains={} permits={}",
        counters.installs,
        counters.evictions,
        counters.cancelled_requests,
        counters.discarded_reads,
        counters.decoded_bytes_installed,
        counters.budget_limited_drains,
        controller.permits_in_use(),
    )
    .unwrap();
}

fn run_schedule() -> String {
    let set = chain_set();
    let stage = CellDemand::new(set.max_lead);
    let mut controller = trace_controller();
    let mut out = String::new();
    writeln!(
        out,
        "# SH controller trace baseline. Produced by\n\
         # sh_streaming::controller::trace_tests::sh_trace_controller_schedule_matches_baseline\n\
         # from the pre-extraction SH streaming controller (brief AC 18).\n\
         # Per frame: completions admitted, drain target deltas, evictions, capacity\n\
         # victims, installs in admission order with decoded bytes against the per-drain\n\
         # budget, ready work left behind, requests in issue order (m = mandatory, o =\n\
         # optional) with class, what the simulated pool did, then the published targets.\n\
         # Do not regenerate to make a refactor pass."
    )
    .unwrap();
    let mut outstanding: Vec<ShClusterRequest> = Vec::new();
    let mut reading = BTreeSet::new();
    let mut pool = SimulatedPool {
        capacity: POOL_CLUSTERS,
        ..SimulatedPool::default()
    };
    for (index, frame) in SCHEDULE.iter().enumerate() {
        writeln!(
            out,
            "frame {index} t={:.1} camera_cell={} visible_cells={} in_flight={}",
            frame.seconds,
            frame.camera_cell,
            ids(frame.visible_cells.iter().copied()),
            ids(frame.in_flight.iter().copied()),
        )
        .unwrap();
        controller.promote_composed_clusters();
        let visible = VisibleCells::Culled(frame.visible_cells.to_vec());
        controller
            .update_targets(
                &visible,
                Some(stage.frame(&set, frame.camera_cell as u32, PORTAL, &visible)),
                frame.seconds,
            )
            .unwrap();
        complete_outstanding(
            &mut controller,
            frame,
            &mut outstanding,
            &mut reading,
            &mut out,
        );
        let (batch, requests) = controller.take_async_drain_batch_and_requests().unwrap();
        record_drain(&controller, &batch, &mut out);
        let issued: Vec<String> = requests
            .iter()
            .map(|request| {
                let class = controller.states[request.cluster_id as usize]
                    .class
                    .map_or("none", class_name);
                let tier = if request.mandatory { "m" } else { "o" };
                format!("{} {tier} {class}", request.cluster_id)
            })
            .collect();
        writeln!(out, "  requests: [{}]", issued.join(", ")).unwrap();
        outstanding.extend(requests);
        let outcome = pool.drain(&batch);
        let evicted = outcome.evicted.iter().copied();
        let refused = outcome.pool_refused.iter().copied();
        writeln!(
            out,
            "  pool: evicted={} pool_refused={} capacity={}",
            ids(evicted),
            ids(refused),
            pool.capacity
        )
        .unwrap();
        controller.apply_drain_outcome(outcome).unwrap();
        record_state(&controller, &mut out);
    }
    out
}

#[test]
fn sh_trace_controller_schedule_matches_baseline() {
    assert_matches_baseline(BASELINE, &run_schedule());
}
